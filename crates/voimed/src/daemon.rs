use std::time::Duration;

use anyhow::Context;
use voime_core::audio::capture::CaptureHandle;
use voime_core::pipeline::{Pipeline, PipelineEvent};
use voime_core::r#final::Final;
use voime_core::vad::segmenter::Segment;
use voime_ipc::{
    ClientMessage, ErrorCode, FrontendState, Mode, Phase, ServerMessage, TriggerSource,
};
use voime_log::vlog;

use crate::backend::BackendId;
use crate::config::Config;
use crate::ipc_server::{IpcServer, ServerEvent};
use crate::state::State;
use crate::worker::{Worker, WorkerStatus};

pub struct Daemon {
    _config: Config,
    state: State,
    pipeline: Pipeline,
    finalizer: Final,
    audio: CaptureHandle,
    ipc: IpcServer,
    backend: BackendId,
    worker: Option<Worker>,
    worker_status: WorkerStatus,
    worker_failures: u32,
    current_segment: Option<u64>,
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
    ) -> Self {
        let backend = config.final_config.backend;
        let worker_status = worker
            .as_ref()
            .map(|worker| worker.status())
            .unwrap_or(WorkerStatus::Unavailable);
        Self {
            _config: config,
            state: State::default(),
            pipeline,
            finalizer,
            audio,
            ipc,
            backend,
            worker,
            worker_status,
            worker_failures: 0,
            current_segment: None,
            next_segment: 1,
            pending_mode: None,
            pending_stop: false,
            session_active: false,
            shutdown: false,
            fatal: None,
        }
    }

    pub fn run(mut self) -> anyhow::Result<()> {
        let ipc_events = self.ipc.events().clone();
        let pipeline_events = self.pipeline.events.clone();
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
                default(Duration::from_millis(100)) => {}
            }
        }

        if let Some(worker) = self.worker.take() {
            worker.shutdown();
        }
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
            ClientMessage::Start { source, .. } => self.request_start(client_id, source),
            ClientMessage::Stop { segment_id, .. } => self.request_stop(client_id, segment_id),
            ClientMessage::Toggle { segment_id, source } => {
                if !matches!(self.effective_mode(), Mode::Ptt | Mode::Toggle) {
                    self.send_error(client_id, ErrorCode::Busy);
                    return;
                }
                if self.session_active {
                    self.request_stop(client_id, segment_id);
                    return;
                }
                if self.state.phase == Phase::Idle {
                    self.request_start(client_id, source);
                } else {
                    self.send_error(client_id, ErrorCode::Busy);
                }
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
        if self.state.phase != Phase::Idle || self.session_active {
            self.send_error(client_id, ErrorCode::Busy);
            return;
        }
        let mode = self.effective_mode();
        self.pending_stop = false;
        if mode != Mode::Continuous {
            self.session_active = true;
        }
        if let Err(error) = self.audio.play().context("启动音频采集失败") {
            self.fail(error);
            return;
        }
        let segment_id = self.allocate_segment();
        self.current_segment = Some(segment_id);
        self.state.phase = Phase::Recording;
        self.state.partial = None;
        if let Err(error) = self.pipeline.start_segment() {
            self.fail(error);
            return;
        }
        self.ipc
            .broadcast(&ServerMessage::SegmentStarted { segment_id, source });
        self.broadcast_state();
        self.sync_audio();
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
                if self.state.phase != Phase::Finalizing {
                    self.state.phase = Phase::Recording;
                }
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
                vlog!(
                    "pipeline segment-ready samples={} {}..{}",
                    segment.samples.len(),
                    segment.start_ms,
                    segment.end_ms
                );
                if self.current_segment.is_none() {
                    if self.state.phase == Phase::Finalizing {
                        self.state.phase = Phase::Idle;
                        self.broadcast_state();
                        self.apply_pending_mode();
                        self.sync_audio();
                    }
                    return;
                }
                self.state.phase = Phase::Finalizing;
                if let Err(error) = self.finalize(segment) {
                    self.drop_current(&format!("{error:#}"));
                }
            }
            PipelineEvent::SegmentDropped { reason } => {
                vlog!("pipeline segment-dropped: {reason}");
                self.drop_current(&reason);
            }
            PipelineEvent::Fatal(reason) => self.fatal = Some(anyhow::anyhow!(reason)),
        }
        if self.fatal.is_some() {
            self.shutdown = true;
        }
    }

    fn finalize(&mut self, segment: Segment) -> anyhow::Result<()> {
        let Some(segment_id) = self.current_segment else {
            return Ok(());
        };
        if segment.samples.is_empty() {
            self.drop_current("空语音段");
            return Ok(());
        }
        if self.state.phase != Phase::Finalizing {
            return Ok(());
        }
        let text = match self.backend {
            BackendId::Cpu => self.transcribe_cpu(&segment.samples),
            BackendId::Firered | BackendId::Qwen3 => self.transcribe_gpu(&segment.samples),
        };
        let text = match text {
            Ok(text) => text,
            Err(error) => {
                vlog!("segment {segment_id} inference failed: {error:#}");
                self.drop_current("推理失败");
                return Ok(());
            }
        };
        if text.trim().is_empty() {
            self.drop_current("空识别结果");
            return Ok(());
        }
        vlog!("final segment {segment_id} chars={}", text.chars().count());
        self.state.transcript.push(text.clone());
        self.state.partial = None;
        self.ipc
            .broadcast(&ServerMessage::Final { segment_id, text });
        self.current_segment = None;
        self.state.phase = Phase::Idle;
        self.broadcast_state();
        if !self.session_active {
            self.apply_pending_mode();
        }
        self.sync_audio();
        Ok(())
    }

    fn transcribe_cpu(&mut self, samples: &[f32]) -> anyhow::Result<String> {
        self.finalizer.transcribe(samples)
    }

    fn transcribe_gpu(&mut self, samples: &[f32]) -> anyhow::Result<String> {
        if self.worker.is_none() || self.worker_status == WorkerStatus::Unavailable {
            return self.fallback_cpu(samples, &anyhow::anyhow!("{} worker 不可用", self.backend));
        }
        let Some(worker) = self.worker.as_mut() else {
            return self.fallback_cpu(samples, &anyhow::anyhow!("{} worker 不可用", self.backend));
        };
        match worker.transcribe(samples) {
            Ok(text) => {
                self.worker_status = worker.status();
                self.worker_failures = 0;
                Ok(text)
            }
            Err(error) => {
                vlog!("gpu worker {} failed: {error:#}", self.backend);
                self.worker_failures = self.worker_failures.saturating_add(1);
                if self.worker_failures >= crate::worker::MAX_RESTARTS {
                    vlog!(
                        "gpu worker {} unavailable: {} consecutive failures",
                        self.backend,
                        self.worker_failures
                    );
                    self.worker_status = WorkerStatus::Unavailable;
                } else {
                    let backoff = std::time::Duration::from_millis(
                        100u64.saturating_mul(1 << (self.worker_failures - 1).min(4)),
                    );
                    std::thread::sleep(backoff);
                    if let Err(restart) = worker.restart() {
                        vlog!("gpu worker {} restart failed: {restart:#}", self.backend);
                        self.worker_status = WorkerStatus::Unavailable;
                    } else {
                        self.worker_status = worker.status();
                    }
                }
                self.fallback_cpu(samples, &error)
            }
        }
    }

    fn fallback_cpu(&mut self, samples: &[f32], error: &anyhow::Error) -> anyhow::Result<String> {
        vlog!("cpu fallback: {error:#}");
        self.finalizer.transcribe(samples)
    }

    fn drop_current(&mut self, reason: &str) {
        let segment_id = self.current_segment.take();
        if let Some(segment_id) = segment_id {
            vlog!("segment {segment_id} dropped: {reason}");
            self.ipc.broadcast(&ServerMessage::Error {
                code: ErrorCode::SegmentDropped,
                segment_id: Some(segment_id),
                message: reason.to_string(),
            });
        }
        self.state.partial = None;
        self.state.phase = Phase::Idle;
        self.broadcast_state();
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
        if self.state.phase != Phase::Idle {
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
        listens(self.effective_mode(), self.session_active)
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

fn listens(mode: Mode, session_active: bool) -> bool {
    mode == Mode::Continuous || session_active
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

    #[test]
    fn continuous_listens_without_session_but_ptt_needs_one() {
        assert!(listens(Mode::Continuous, false));
        assert!(!listens(Mode::Ptt, false));
        assert!(listens(Mode::Ptt, true));
        assert!(!listens(Mode::Toggle, false));
        assert!(listens(Mode::Toggle, true));
    }
}
