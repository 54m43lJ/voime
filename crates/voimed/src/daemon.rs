use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use anyhow::Context;
use voime_core::audio::capture::CaptureHandle;
use voime_core::pipeline::{Pipeline, PipelineEvent};
use voime_core::r#final::Final;
use voime_core::vad::segmenter::Segment;
use voime_ipc::{
    ClientMessage, ErrorCode, FrontendState, Mode, Phase, Role, ServerMessage, TriggerSource,
    WorkerDiagnostics, WorkerState,
};
use voime_log::vlog;

use crate::backend::BackendId;
use crate::config::Config;
use crate::ipc_server::{IpcServer, ServerEvent};
use crate::state::State;
use crate::worker::{ReadyInfo, Worker, WorkerStatus};

pub const FINALIZE_QUEUE_CAPACITY: usize = 5;

struct FinalizeJob {
    segment_id: u64,
    segment: Segment,
}

struct FinalizeResult {
    segment_id: u64,
    result: Result<String, String>,
}

struct FinalizeQueue {
    jobs: VecDeque<FinalizeJob>,
    inflight: bool,
}

impl FinalizeQueue {
    fn new() -> Self {
        Self {
            jobs: VecDeque::new(),
            inflight: false,
        }
    }

    fn pending(&self) -> usize {
        self.jobs.len() + usize::from(self.inflight)
    }

    fn push(&mut self, job: FinalizeJob) -> Option<FinalizeJob> {
        let dropped = if self.jobs.len() >= FINALIZE_QUEUE_CAPACITY {
            self.jobs.pop_front()
        } else {
            None
        };
        self.jobs.push_back(job);
        dropped
    }

    fn next(&mut self) -> Option<FinalizeJob> {
        if self.inflight {
            return None;
        }
        let job = self.jobs.pop_front();
        if job.is_some() {
            self.inflight = true;
        }
        job
    }

    fn complete(&mut self) {
        self.inflight = false;
    }
}

#[derive(Clone)]
struct WorkerShared {
    status: Arc<Mutex<WorkerStatus>>,
    ready: Arc<Mutex<Option<ReadyInfo>>>,
    error: Arc<Mutex<Option<String>>>,
}

impl WorkerShared {
    fn new(status: WorkerStatus, ready: Option<ReadyInfo>, error: Option<String>) -> Self {
        Self {
            status: Arc::new(Mutex::new(status)),
            ready: Arc::new(Mutex::new(ready)),
            error: Arc::new(Mutex::new(error)),
        }
    }

    fn update(&self, status: WorkerStatus, ready: Option<ReadyInfo>, error: Option<String>) {
        *lock(&self.status) = status;
        *lock(&self.ready) = ready;
        *lock(&self.error) = error;
    }

    fn snapshot(&self) -> (WorkerStatus, Option<ReadyInfo>, Option<String>) {
        (
            *lock(&self.status),
            lock(&self.ready).clone(),
            lock(&self.error).clone(),
        )
    }
}

struct Finalizer {
    jobs: Option<crossbeam_channel::Sender<FinalizeJob>>,
    results: crossbeam_channel::Receiver<FinalizeResult>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Finalizer {
    fn spawn(
        backend: BackendId,
        finalizer: Final,
        worker: Option<Worker>,
        shared: WorkerShared,
    ) -> anyhow::Result<Self> {
        let (jobs_tx, jobs_rx) = crossbeam_channel::unbounded();
        let (results_tx, results_rx) = crossbeam_channel::unbounded();
        let stop = Arc::new(AtomicBool::new(false));
        let thread_stop = Arc::clone(&stop);
        let thread = std::thread::Builder::new()
            .name("voime-finalizer".to_string())
            .spawn(move || {
                finalize_loop(
                    backend,
                    finalizer,
                    worker,
                    shared,
                    jobs_rx,
                    results_tx,
                    thread_stop,
                );
            })
            .context("创建 Finalizer 线程失败")?;
        Ok(Self {
            jobs: Some(jobs_tx),
            results: results_rx,
            stop,
            thread: Some(thread),
        })
    }

    fn submit(&self, job: FinalizeJob) -> anyhow::Result<()> {
        let sender = self.jobs.as_ref().context("Finalizer 已关闭")?;
        sender
            .send(job)
            .map_err(|_| anyhow::anyhow!("Finalizer 线程已退出"))
    }

    fn results(&self) -> &crossbeam_channel::Receiver<FinalizeResult> {
        &self.results
    }

    fn shutdown(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        self.jobs.take();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn finalize_loop(
    backend: BackendId,
    mut finalizer: Final,
    mut worker: Option<Worker>,
    shared: WorkerShared,
    jobs: crossbeam_channel::Receiver<FinalizeJob>,
    results: crossbeam_channel::Sender<FinalizeResult>,
    stop: Arc<AtomicBool>,
) {
    let mut worker_status = worker
        .as_ref()
        .map(Worker::status)
        .unwrap_or(WorkerStatus::Unavailable);
    let mut worker_error = shared.snapshot().2;
    let mut worker_failures = 0;
    loop {
        if stop.load(Ordering::Relaxed) {
            break;
        }
        match jobs.recv_timeout(Duration::from_millis(500)) {
            Ok(job) => {
                if stop.load(Ordering::Relaxed) {
                    break;
                }
                let result = transcribe(
                    backend,
                    &mut finalizer,
                    &mut worker,
                    &mut worker_status,
                    &mut worker_error,
                    &mut worker_failures,
                    &job.segment.samples,
                );
                let ready = if worker_status == WorkerStatus::Unavailable {
                    None
                } else {
                    worker
                        .as_ref()
                        .and_then(|worker| worker.ready_info().cloned())
                };
                shared.update(worker_status, ready, worker_error.clone());
                if results
                    .send(FinalizeResult {
                        segment_id: job.segment_id,
                        result: result.map_err(|error| format!("{error:#}")),
                    })
                    .is_err()
                {
                    break;
                }
            }
            Err(crossbeam_channel::RecvTimeoutError::Timeout) => {
                if let Some(worker) = worker.as_mut() {
                    let alive = worker.is_alive();
                    if !alive {
                        worker_status = worker.status();
                        worker_error = worker.last_error().map(str::to_string);
                    }
                    let ready = if alive {
                        worker.ready_info().cloned()
                    } else {
                        None
                    };
                    shared.update(worker_status, ready, worker_error.clone());
                }
            }
            Err(crossbeam_channel::RecvTimeoutError::Disconnected) => break,
        }
    }
    if let Some(worker) = worker.take() {
        worker.shutdown();
    }
}

fn transcribe(
    backend: BackendId,
    finalizer: &mut Final,
    worker: &mut Option<Worker>,
    worker_status: &mut WorkerStatus,
    worker_error: &mut Option<String>,
    worker_failures: &mut u32,
    samples: &[f32],
) -> anyhow::Result<String> {
    match backend {
        BackendId::Cpu => finalizer.transcribe(samples),
        BackendId::Qwen3 => transcribe_gpu(
            backend,
            finalizer,
            worker,
            worker_status,
            worker_error,
            worker_failures,
            samples,
        ),
    }
}

fn transcribe_gpu(
    backend: BackendId,
    finalizer: &mut Final,
    worker: &mut Option<Worker>,
    worker_status: &mut WorkerStatus,
    worker_error: &mut Option<String>,
    worker_failures: &mut u32,
    samples: &[f32],
) -> anyhow::Result<String> {
    if worker.is_none() || *worker_status == WorkerStatus::Unavailable {
        return fallback_cpu(
            finalizer,
            samples,
            anyhow::anyhow!("{} worker 不可用", backend),
        );
    }
    let Some(worker) = worker.as_mut() else {
        unreachable!("worker checked");
    };
    match worker.transcribe(samples) {
        Ok(text) => {
            *worker_status = worker.status();
            *worker_error = None;
            *worker_failures = 0;
            Ok(text)
        }
        Err(error) => {
            vlog!("gpu worker {} failed: {error:#}", backend);
            *worker_error = Some(format!("{error:#}"));
            *worker_failures = worker_failures.saturating_add(1);
            if *worker_failures >= crate::worker::MAX_RESTARTS {
                vlog!(
                    "gpu worker {} unavailable: {} consecutive failures",
                    backend,
                    *worker_failures
                );
                *worker_status = WorkerStatus::Unavailable;
            } else {
                let backoff = Duration::from_millis(
                    100u64.saturating_mul(1 << (*worker_failures - 1).min(4)),
                );
                std::thread::sleep(backoff);
                if let Err(restart) = worker.restart() {
                    vlog!("gpu worker {} restart failed: {restart:#}", backend);
                    *worker_status = WorkerStatus::Unavailable;
                    *worker_error = Some(format!("{restart:#}"));
                } else {
                    *worker_status = worker.status();
                    *worker_error = None;
                }
            }
            fallback_cpu(finalizer, samples, error)
        }
    }
}

fn fallback_cpu(
    finalizer: &mut Final,
    samples: &[f32],
    error: anyhow::Error,
) -> anyhow::Result<String> {
    vlog!("cpu fallback: {error:#}");
    let started = Instant::now();
    let result = finalizer.transcribe(samples);
    let elapsed = started.elapsed().as_millis() as u64;
    match &result {
        Ok(text) => vlog!(
            "cpu fallback result latency_ms={elapsed} chars={}",
            text.chars().count()
        ),
        Err(cpu_error) => vlog!("cpu fallback failed after {elapsed} ms: {cpu_error:#}"),
    }
    result
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

pub struct Daemon {
    _config: Config,
    state: State,
    pipeline: Pipeline,
    finalizer: Finalizer,
    worker_shared: WorkerShared,
    audio: CaptureHandle,
    ipc: IpcServer,
    backend: BackendId,
    current_segment: Option<u64>,
    awaiting_segments: VecDeque<u64>,
    finalize: FinalizeQueue,
    next_segment: u64,
    pending_mode: Option<Mode>,
    pending_stop: bool,
    session_active: bool,
    shutdown: bool,
    fatal: Option<anyhow::Error>,
}

impl Daemon {
    pub fn new(
        config: Config,
        pipeline: Pipeline,
        finalizer: Final,
        audio: CaptureHandle,
        ipc: IpcServer,
        worker: Option<Worker>,
        worker_error: Option<String>,
    ) -> anyhow::Result<Self> {
        let backend = config.final_config.backend;
        let worker_status = worker
            .as_ref()
            .map(|worker| worker.status())
            .unwrap_or(WorkerStatus::Unavailable);
        let ready = worker
            .as_ref()
            .and_then(|worker| worker.ready_info().cloned());
        let worker_shared = WorkerShared::new(worker_status, ready, worker_error);
        let finalizer = Finalizer::spawn(backend, finalizer, worker, worker_shared.clone())?;
        Ok(Self {
            _config: config,
            state: State::default(),
            pipeline,
            finalizer,
            worker_shared,
            audio,
            ipc,
            backend,
            current_segment: None,
            awaiting_segments: VecDeque::new(),
            finalize: FinalizeQueue::new(),
            next_segment: 1,
            pending_mode: None,
            pending_stop: false,
            session_active: false,
            shutdown: false,
            fatal: None,
        })
    }

    pub fn run(mut self) -> anyhow::Result<()> {
        let ipc_events = self.ipc.events().clone();
        let pipeline_events = self.pipeline.events.clone();
        let finalize_events = self.finalizer.results().clone();
        if let Err(error) = self.sync_audio_result() {
            self.fail(error);
        }

        while !self.shutdown {
            crossbeam_channel::select! {
                recv(ipc_events) -> message => match message {
                    Ok(event) => self.on_server_event(event),
                    Err(_) => break,
                },
                recv(pipeline_events) -> message => match message {
                    Ok(event) => self.on_pipeline(event),
                    Err(_) => break,
                },
                recv(finalize_events) -> message => match message {
                    Ok(result) => self.on_finalize_result(result),
                    Err(_) => break,
                },
                default(Duration::from_millis(100)) => {}
            }
        }

        self.finalizer.shutdown();
        self.ipc.broadcast(&ServerMessage::Bye {});
        // Give the writer threads a chance to flush the final frame before
        // the streams are shut down.
        std::thread::sleep(Duration::from_millis(50));
        self.ipc.shutdown();
        if let Err(error) = self.pipeline.shutdown() {
            if self.fatal.is_none() {
                self.fatal = Some(error);
            }
        }
        self.audio.stop();
        match self.fatal.take() {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }

    fn on_server_event(&mut self, event: ServerEvent) {
        match event {
            ServerEvent::Connected { client_id } => {
                vlog!("client {client_id} connected");
            }
            ServerEvent::Message { client_id, message } => self.on_client(client_id, message),
            ServerEvent::BadMessage { client_id, message } => {
                vlog!("client {client_id} bad message: {message}");
                self.send_error(client_id, ErrorCode::InvalidMessage);
                self.ipc.disconnect(client_id);
            }
            ServerEvent::Disconnected { client_id, role } => {
                vlog!("client {client_id} disconnected");
                // Drop the stale entry before broadcasting. Otherwise a
                // reconnecting frontend would be rejected as a role conflict.
                self.ipc.disconnect(client_id);
                if role == Some(voime_ipc::Role::Frontend) {
                    self.on_frontend_disconnected();
                }
            }
        }
    }

    fn on_client(&mut self, client_id: u64, message: ClientMessage) {
        let role = self.ipc.role(client_id);
        if role.is_none() && !matches!(message, ClientMessage::Hello { .. }) {
            self.send_error(client_id, ErrorCode::InvalidMessage);
            self.ipc.disconnect(client_id);
            return;
        }

        // TUI is a debug/monitor client. It may drive daemon sessions only
        // while no platform frontend (and therefore no input-method target)
        // is connected; otherwise it is observe-only.
        let tui_session_blocked = tui_session_commands_blocked(role, self.state.addon);

        match message {
            ClientMessage::Hello {
                role: requested,
                version,
                config,
            } => {
                if role.is_some() {
                    self.send_error(client_id, ErrorCode::InvalidMessage);
                    self.ipc.disconnect(client_id);
                    return;
                }
                if version != voime_ipc::PROTOCOL_VERSION {
                    self.send_error(client_id, ErrorCode::VersionMismatch);
                    self.ipc.disconnect(client_id);
                    return;
                }
                if !self.ipc.set_role(client_id, requested) {
                    self.send_error(client_id, ErrorCode::RoleConflict);
                    self.ipc.disconnect(client_id);
                    return;
                }
                if requested == voime_ipc::Role::Frontend {
                    self.state.addon = FrontendState::Connected;
                    if let Some(config) = config {
                        self.apply_frontend_config(config.mode, config.hotkey);
                    }
                    self.broadcast_snapshot();
                } else {
                    self.ipc.send_to(client_id, &self.state.snapshot());
                }
            }
            ClientMessage::Configure { mode, hotkey } => {
                if role != Some(voime_ipc::Role::Frontend) {
                    self.send_error(client_id, ErrorCode::InvalidMessage);
                    return;
                }
                self.apply_frontend_config(mode, hotkey);
                self.broadcast_snapshot();
            }
            ClientMessage::Start { source, .. } => {
                if tui_session_blocked {
                    self.send_error(client_id, ErrorCode::Busy);
                    return;
                }
                self.request_start(client_id, source);
            }
            ClientMessage::Stop { segment_id, .. } => {
                if tui_session_blocked {
                    self.send_error(client_id, ErrorCode::Busy);
                    return;
                }
                self.request_stop(client_id, segment_id);
            }
            ClientMessage::Toggle { segment_id, source } => {
                if tui_session_blocked {
                    self.send_error(client_id, ErrorCode::Busy);
                    return;
                }
                if !matches!(self.effective_mode(), Mode::Ptt | Mode::Toggle) {
                    self.send_error(client_id, ErrorCode::Busy);
                    return;
                }
                if self.session_active {
                    self.request_stop(client_id, segment_id);
                    return;
                }
                self.request_start(client_id, source);
            }
            ClientMessage::ClearHistory {} => {
                self.state.clear_history();
                self.ipc.broadcast(&ServerMessage::HistoryCleared {});
            }
            ClientMessage::Status {} => {
                let status = self.state.status(
                    self.state.phase == Phase::Recording,
                    self.state.phase == Phase::Finalizing,
                    self.current_segment,
                );
                self.ipc
                    .send_to(client_id, &ServerMessage::Status { status });
            }
            ClientMessage::Diagnostics {} => {
                if role != Some(voime_ipc::Role::Control) {
                    self.send_error(client_id, ErrorCode::InvalidMessage);
                    return;
                }
                let diagnostics = self.diagnostics();
                self.ipc.send_to(client_id, &diagnostics);
            }
            ClientMessage::Quit {} => {
                if role != Some(voime_ipc::Role::Control) {
                    self.send_error(client_id, ErrorCode::InvalidMessage);
                    return;
                }
                self.request_shutdown();
            }
        }
    }

    fn apply_frontend_config(&mut self, mode: Mode, hotkey: String) {
        self.state.hotkey = Some(hotkey);
        self.pending_stop = false;
        if self.state.mode == Some(mode) {
            return;
        }
        if self.state.phase == Phase::Idle && !self.session_active {
            self.state.apply_mode(mode);
            self.apply_pipeline_mode();
            self.sync_audio();
            return;
        }
        vlog!(
            "mode change requested mode={mode:?} phase={:?} session={}",
            self.state.phase,
            self.session_active
        );
        self.pending_mode = Some(mode);
        // A configuration mode change ends the old PTT / Toggle session; the
        // new mode becomes effective after the old segment's Final / drop.
        self.pending_stop = true;
        self.session_active = false;
        if self.state.phase == Phase::Recording {
            self.state.phase = Phase::Finalizing;
            if let Err(error) = self
                .pipeline
                .finish_segment()
                .and_then(|_| self.pipeline.cancel_preview())
            {
                self.fail(error);
                return;
            }
            self.broadcast_state();
        } else if self.current_segment.is_none() {
            // Between soft-split segments there is nothing to finalize; the
            // queued continuation must not enter the new mode.
            self.apply_pending_mode();
        }
        self.sync_audio();
    }

    fn request_start(&mut self, client_id: u64, source: TriggerSource) {
        if self.session_active {
            self.send_error(client_id, ErrorCode::Busy);
            return;
        }
        if self.state.phase == Phase::Recording {
            self.send_error(client_id, ErrorCode::Busy);
            return;
        }
        // A hard stop may have ended the session before the pipeline has
        // delivered the old SegmentReady. Detach its ID so the new session can
        // start recording while finalization continues in the background.
        if let Some(segment_id) = self.current_segment.take() {
            self.awaiting_segments.push_back(segment_id);
        }
        if let Err(error) = self.start_session(source) {
            self.fail(error);
        }
    }

    fn start_session(&mut self, source: TriggerSource) -> anyhow::Result<()> {
        let mode = self.effective_mode();
        self.pending_stop = false;
        if mode != Mode::Continuous {
            self.session_active = true;
        }
        self.audio.play().context("启动音频采集失败")?;
        let segment_id = self.allocate_segment();
        self.current_segment = Some(segment_id);
        self.state.phase = Phase::Recording;
        self.state.partial = None;
        self.pipeline.start_segment()?;
        self.ipc
            .broadcast(&ServerMessage::SegmentStarted { segment_id, source });
        self.broadcast_state();
        self.sync_audio();
        Ok(())
    }

    fn request_stop(&mut self, client_id: u64, segment_id: u64) {
        let mode = self.effective_mode();
        if mode == Mode::Continuous {
            if segment_id != 0 && Some(segment_id) != self.current_segment {
                self.send_error(client_id, ErrorCode::Busy);
                return;
            }
            if self.current_segment.is_none() {
                return;
            }
            if self.state.phase == Phase::Recording {
                self.state.phase = Phase::Finalizing;
                if let Err(error) = self
                    .pipeline
                    .finish_segment()
                    .and_then(|_| self.pipeline.cancel_preview())
                {
                    self.fail(error);
                    return;
                }
            }
            self.broadcast_state();
            self.sync_audio();
            return;
        }

        if !self.session_active {
            return;
        }
        self.session_active = false;
        self.pending_stop = true;
        if self.state.phase == Phase::Recording {
            self.state.phase = Phase::Finalizing;
            if let Err(error) = self
                .pipeline
                .finish_segment()
                .and_then(|_| self.pipeline.cancel_preview())
            {
                self.fail(error);
                return;
            }
        }
        self.broadcast_state();
        if self.state.phase == Phase::Idle {
            self.apply_pending_mode();
        }
        self.sync_audio();
    }

    fn on_pipeline(&mut self, event: PipelineEvent) {
        match event {
            PipelineEvent::Recording(true) => {
                vlog!("pipeline recording=true");
                if !listens(self.effective_mode(), self.session_active) || self.pending_stop {
                    self.pending_stop = false;
                    if let Err(error) = self
                        .pipeline
                        .finish_segment()
                        .and_then(|_| self.pipeline.cancel_preview())
                    {
                        self.fail(error);
                    }
                    return;
                }
                if self.current_segment.is_none() {
                    let segment_id = self.allocate_segment();
                    self.current_segment = Some(segment_id);
                    self.state.partial = None;
                    self.ipc.broadcast(&ServerMessage::SegmentStarted {
                        segment_id,
                        source: continuation_source(self.state.mode),
                    });
                }
                self.state.phase = Phase::Recording;
                self.broadcast_state();
            }
            PipelineEvent::Recording(false) => {
                vlog!("pipeline recording=false");
                if self.state.phase == Phase::Recording && self.current_segment.is_some() {
                    self.state.phase = Phase::Finalizing;
                    self.broadcast_state();
                }
            }
            PipelineEvent::Partial(text) => {
                vlog!("pipeline partial chars={}", text.chars().count());
                if self.state.phase == Phase::Recording {
                    if let Some(segment_id) = self.current_segment {
                        self.state.partial = Some(text.clone());
                        self.ipc
                            .broadcast(&ServerMessage::Partial { segment_id, text });
                    }
                }
            }
            PipelineEvent::SegmentReady(segment) => {
                let Some(segment_id) = self.take_ready_segment() else {
                    vlog!("pipeline segment-ready without active segment");
                    self.refresh_finalize_phase();
                    return;
                };
                vlog!(
                    "pipeline segment-ready segment={segment_id} samples={} {}..{}",
                    segment.samples.len(),
                    segment.start_ms,
                    segment.end_ms
                );
                if segment.samples.is_empty() {
                    self.broadcast_dropped(segment_id, "空语音段");
                    self.refresh_finalize_phase();
                    return;
                }
                self.enqueue_finalize(FinalizeJob {
                    segment_id,
                    segment,
                });
            }
            PipelineEvent::SegmentDropped { reason } => {
                vlog!("pipeline segment-dropped: {reason}");
                if let Some(segment_id) = self.take_ready_segment() {
                    self.broadcast_dropped(segment_id, &reason);
                }
                if self.current_segment.is_none() {
                    self.state.partial = None;
                }
                self.refresh_finalize_phase();
            }
            PipelineEvent::Fatal(reason) => self.fatal = Some(anyhow::anyhow!(reason)),
        }
        if self.fatal.is_some() {
            self.shutdown = true;
        }
    }

    fn take_ready_segment(&mut self) -> Option<u64> {
        self.awaiting_segments
            .pop_front()
            .or_else(|| self.current_segment.take())
    }

    fn enqueue_finalize(&mut self, job: FinalizeJob) {
        if let Some(dropped) = self.finalize.push(job) {
            self.broadcast_dropped(dropped.segment_id, "Final 队列已满");
        }
        self.start_next_finalize();
        self.refresh_finalize_phase();
    }

    fn start_next_finalize(&mut self) {
        if let Some(job) = self.finalize.next() {
            if let Err(error) = self.finalizer.submit(job) {
                self.fail(error);
            }
        }
    }

    fn on_finalize_result(&mut self, result: FinalizeResult) {
        self.finalize.complete();
        match result.result {
            Ok(text) if !text.trim().is_empty() => {
                vlog!(
                    "final segment {} chars={}",
                    result.segment_id,
                    text.chars().count()
                );
                self.state.transcript.push(text.clone());
                self.ipc.broadcast(&ServerMessage::Final {
                    segment_id: result.segment_id,
                    text,
                });
            }
            Ok(_) => self.broadcast_dropped(result.segment_id, "空识别结果"),
            Err(error) => self.broadcast_dropped(result.segment_id, &format!("推理失败：{error}")),
        }
        if self.current_segment.is_none() {
            self.state.partial = None;
        }
        self.start_next_finalize();
        self.refresh_finalize_phase();
    }

    fn broadcast_dropped(&mut self, segment_id: u64, reason: &str) {
        vlog!("segment {segment_id} dropped: {reason}");
        self.ipc.broadcast(&ServerMessage::Error {
            code: ErrorCode::SegmentDropped,
            segment_id: Some(segment_id),
            message: reason.to_string(),
        });
    }

    fn refresh_finalize_phase(&mut self) {
        if self.current_segment.is_some() {
            if self.state.phase != Phase::Recording {
                self.state.phase = Phase::Recording;
                self.broadcast_state();
            }
            return;
        }
        if self.finalize.pending() > 0 {
            if self.state.phase != Phase::Finalizing {
                self.state.phase = Phase::Finalizing;
                self.broadcast_state();
            }
            return;
        }
        if self.state.phase != Phase::Idle {
            self.state.phase = Phase::Idle;
            self.broadcast_state();
        }
        if !self.session_active {
            self.apply_pending_mode();
        }
        self.sync_audio();
    }

    fn on_frontend_disconnected(&mut self) {
        vlog!("frontend disconnected");
        self.state.addon = FrontendState::Disconnected;
        self.session_active = false;
        self.pending_stop = false;
        self.pending_mode = None;
        if self.state.mode != Some(Mode::Ptt) {
            self.state.apply_mode(Mode::Ptt);
        }
        if self.current_segment.is_some() {
            self.state.phase = Phase::Finalizing;
            let _ = self.pipeline.finish_segment();
            let _ = self.pipeline.cancel_preview();
        }
        self.apply_pipeline_mode();
        self.sync_audio();
        self.broadcast_snapshot();
    }

    fn allocate_segment(&mut self) -> u64 {
        let id = self.next_segment;
        self.next_segment = self.next_segment.wrapping_add(1);
        if self.next_segment == 0 {
            self.next_segment = 1;
        }
        id
    }

    fn broadcast_state(&self) {
        self.ipc.broadcast(&ServerMessage::State {
            recording: self.state.phase == Phase::Recording,
            finalizing: self.state.phase == Phase::Finalizing,
            segment_id: self.current_segment,
        });
    }

    fn broadcast_snapshot(&self) {
        self.ipc.broadcast(&self.state.snapshot());
    }

    fn apply_pipeline_mode(&self) {
        if let Err(error) = self.pipeline.set_mode(self.effective_mode()) {
            vlog!("set pipeline mode failed: {error:#}");
        }
    }

    fn apply_pending_mode(&mut self) {
        if self.state.phase != Phase::Idle || self.finalize.pending() > 0 {
            return;
        }
        if let Some(mode) = self.pending_mode.take() {
            vlog!("apply pending mode {mode:?}");
            self.pending_stop = false;
            self.session_active = false;
            self.state.apply_mode(mode);
            self.apply_pipeline_mode();
            self.sync_audio();
        }
    }

    fn effective_mode(&self) -> Mode {
        if self.state.addon == FrontendState::Connected {
            self.state.mode.unwrap_or(Mode::Ptt)
        } else {
            Mode::Ptt
        }
    }

    fn audio_should_play(&self) -> bool {
        capture_needed(self.effective_mode(), self.session_active, self.state.phase)
    }

    fn sync_audio(&mut self) {
        if let Err(error) = self.sync_audio_result() {
            self.fail(error);
        }
    }

    fn sync_audio_result(&mut self) -> anyhow::Result<()> {
        if self.audio_should_play() {
            self.audio.play().context("启动音频采集失败")
        } else {
            self.audio.pause().context("暂停音频采集失败")
        }
    }

    /// Read-only worker snapshot used by `voime doctor`. It reflects daemon
    /// state only; it never probes or restarts a worker.
    fn diagnostics(&mut self) -> ServerMessage {
        let backend = self.backend.as_str().to_string();
        if self.backend == BackendId::Cpu {
            return ServerMessage::Diagnostics {
                backend,
                worker: None,
            };
        }
        let (status, ready, error) = self.worker_shared.snapshot();
        ServerMessage::Diagnostics {
            backend,
            worker: Some(WorkerDiagnostics {
                state: worker_state(status),
                runtime: ready.as_ref().map(|info| info.runtime.clone()),
                device: ready.as_ref().map(|info| info.device.clone()),
                vram_mb: ready.as_ref().map(|info| info.vram_mb).unwrap_or(0),
                punc: ready.as_ref().map(|info| info.punc).unwrap_or(false),
                error,
            }),
        }
    }

    fn request_shutdown(&mut self) {
        self.shutdown = true;
    }

    fn send_error(&self, client_id: u64, code: ErrorCode) {
        self.ipc.send_to(
            client_id,
            &ServerMessage::Error {
                code,
                segment_id: None,
                message: error_message(code).to_string(),
            },
        );
    }

    fn fail(&mut self, error: anyhow::Error) {
        vlog!("fatal: {error:#}");
        self.fatal = Some(error);
        self.shutdown = true;
    }
}

fn worker_state(status: WorkerStatus) -> WorkerState {
    match status {
        WorkerStatus::Starting => WorkerState::Starting,
        WorkerStatus::Ready => WorkerState::Ready,
        WorkerStatus::Running => WorkerState::Running,
        WorkerStatus::Unavailable => WorkerState::Unavailable,
    }
}

fn listens(mode: Mode, session_active: bool) -> bool {
    mode == Mode::Continuous || session_active
}

fn capture_needed(mode: Mode, session_active: bool, phase: Phase) -> bool {
    listens(mode, session_active) || phase == Phase::Finalizing
}

fn tui_session_commands_blocked(role: Option<Role>, addon: FrontendState) -> bool {
    role == Some(Role::Tui) && addon == FrontendState::Connected
}

fn continuation_source(mode: Option<Mode>) -> TriggerSource {
    match mode {
        Some(Mode::Continuous) => TriggerSource::Continuous,
        Some(Mode::Toggle) => TriggerSource::Toggle,
        _ => TriggerSource::Hold,
    }
}

fn error_message(code: ErrorCode) -> &'static str {
    match code {
        ErrorCode::InvalidMessage => "协议消息非法",
        ErrorCode::VersionMismatch => "协议版本不匹配",
        ErrorCode::RoleConflict => "已有前端连接",
        ErrorCode::Busy => "当前忙",
        ErrorCode::NoInputContext => "没有可用的输入目标",
        ErrorCode::DaemonUnavailable => "daemon 不可用",
        ErrorCode::Unavailable => "服务暂时不可用",
        ErrorCode::SegmentDropped => "语音段已丢弃",
        ErrorCode::Inference => "推理失败",
        ErrorCode::Internal => "内部错误",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn continuation_source_follows_mode() {
        assert_eq!(
            continuation_source(Some(Mode::Continuous)),
            TriggerSource::Continuous
        );
        assert_eq!(
            continuation_source(Some(Mode::Toggle)),
            TriggerSource::Toggle
        );
        assert_eq!(continuation_source(None), TriggerSource::Hold);
    }

    fn finalize_job(segment_id: u64) -> FinalizeJob {
        FinalizeJob {
            segment_id,
            segment: Segment {
                samples: vec![0.0; 1],
                start_ms: 0,
                end_ms: 10,
            },
        }
    }

    #[test]
    fn finalize_queue_keeps_at_most_five_waiting() {
        let mut queue = FinalizeQueue::new();
        for segment_id in 1..=6 {
            queue.push(finalize_job(segment_id));
        }
        assert_eq!(queue.pending(), 5);
        let first = queue.next().expect("queue not empty");
        assert_eq!(first.segment_id, 2);
        assert_eq!(queue.pending(), 5);
        queue.complete();
        let second = queue.next().expect("queue not empty");
        assert_eq!(second.segment_id, 3);
    }

    #[test]
    fn continuous_listens_without_session_but_ptt_needs_one() {
        assert!(listens(Mode::Continuous, false));
        assert!(!listens(Mode::Ptt, false));
        assert!(listens(Mode::Ptt, true));
        assert!(!listens(Mode::Toggle, false));
        assert!(listens(Mode::Toggle, true));
    }

    #[test]
    fn finalizing_keeps_capture_alive() {
        assert!(capture_needed(Mode::Ptt, false, Phase::Finalizing));
        assert!(capture_needed(Mode::Toggle, false, Phase::Finalizing));
        assert!(!capture_needed(Mode::Ptt, false, Phase::Idle));
        assert!(capture_needed(Mode::Ptt, true, Phase::Recording));
        assert!(capture_needed(Mode::Continuous, false, Phase::Idle));
    }

    #[test]
    fn tui_session_commands_blocked_while_frontend_connected() {
        assert!(!tui_session_commands_blocked(
            Some(Role::Tui),
            FrontendState::Disconnected
        ));
        assert!(tui_session_commands_blocked(
            Some(Role::Tui),
            FrontendState::Connected
        ));
        assert!(!tui_session_commands_blocked(
            Some(Role::Control),
            FrontendState::Connected
        ));
        assert!(!tui_session_commands_blocked(
            Some(Role::Frontend),
            FrontendState::Connected
        ));
        assert!(!tui_session_commands_blocked(
            None,
            FrontendState::Connected
        ));
    }

    #[test]
    fn worker_states_map_to_ipc_diagnostics() {
        assert_eq!(worker_state(WorkerStatus::Starting), WorkerState::Starting);
        assert_eq!(worker_state(WorkerStatus::Ready), WorkerState::Ready);
        assert_eq!(worker_state(WorkerStatus::Running), WorkerState::Running);
        assert_eq!(
            worker_state(WorkerStatus::Unavailable),
            WorkerState::Unavailable
        );
    }
}
