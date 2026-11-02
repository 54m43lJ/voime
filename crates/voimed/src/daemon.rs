use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::Context;
use voime_core::asr::r#final::{FinalBackend, SenseVoiceBackend};
use voime_core::audio::capture::CaptureHandle;
use voime_core::config::{self, Args, Config};
use voime_core::infer::session::SessionFactory;
use voime_core::model::BackendKind;
use voime_core::pipeline::{self, FinalUpdate, Pipeline, PipelineEvent};
use voime_ipc::{
    BackendPhase, ClientMessage, ErrorCode, FrontendState, Mode, ModelInfo, Phase, Role,
    ServerMessage, TriggerSource,
};
use voime_log::vlog;

use crate::ipc_server::{IpcServer, ServerEvent};
use crate::models::ModelCatalog;
use crate::state::State;
use crate::worker::WorkerBackend;

type FinalHandle = Arc<Mutex<WorkerBackend>>;
type FinalBuild = (Box<dyn FinalBackend>, Option<FinalHandle>);

pub struct FinalRuntime {
    pub pipeline: Option<Pipeline>,
    factory: SessionFactory,
    args: Args,
    fallback: bool,
}

pub struct Daemon {
    config: Config,
    state: State,
    final_runtime: FinalRuntime,
    models: ModelCatalog,
    worker: Option<FinalHandle>,
    audio: CaptureHandle,
    ipc: IpcServer,
    running_id: String,
    gpu_fallback: bool,
    current_segment: Option<u64>,
    next_segment: u64,
    pending_mode: Option<Mode>,
    pending_stop: bool,
    session_active: bool,
    shutdown: bool,
    fatal: Option<anyhow::Error>,
}

/// Builds the Final backend, pipeline and (for worker packages) the GPU
/// process. Audio capture is started by the caller after this succeeds.
pub fn build_runtime(
    config: &Config,
    args: Args,
    factory: SessionFactory,
    audio_rx: crossbeam_channel::Receiver<voime_core::audio::capture::AudioChunk>,
) -> anyhow::Result<(Config, FinalRuntime, Option<FinalHandle>)> {
    let (backend, worker, fallback) = match build_final(config, &factory) {
        Ok((backend, worker)) => (backend, worker, false),
        Err(error) if config.manifest.backend.kind == BackendKind::Worker => {
            vlog!("GPU Final 初始化失败，回退 CPU SenseVoice：{error:#}");
            let backend = SenseVoiceBackend::new_with_factory(
                &config.preview_manifest.asr,
                &factory,
                pipeline::fbank_config(&config.preview_manifest.fbank),
            )
            .context("GPU 不可用且 CPU SenseVoice 回退失败")?;
            (Box::new(backend) as Box<dyn FinalBackend>, None, true)
        }
        Err(error) => return Err(error),
    };
    let runtime_config = if fallback {
        let mut runtime_config = config.clone();
        runtime_config.model_id = voime_core::model::PREVIEW_PACKAGE_ID.to_string();
        runtime_config.manifest = config.preview_manifest.clone();
        runtime_config
    } else {
        config.clone()
    };
    let pipeline = pipeline::spawn(&runtime_config, factory.clone(), backend, audio_rx)
        .context("加载语音模型失败")?;
    Ok((
        runtime_config,
        FinalRuntime {
            pipeline: Some(pipeline),
            factory,
            args,
            fallback,
        },
        worker,
    ))
}

fn build_final(config: &Config, factory: &SessionFactory) -> anyhow::Result<FinalBuild> {
    match config.manifest.backend.kind {
        BackendKind::Builtin => {
            let backend = SenseVoiceBackend::new_with_factory(
                &config.manifest.asr,
                factory,
                pipeline::fbank_config(&config.manifest.fbank),
            )
            .context("加载 CPU SenseVoice Final 失败")?;
            Ok((Box::new(backend), None))
        }
        BackendKind::Worker => {
            let device = config.manifest.backend.device.as_deref().unwrap_or("auto");
            let mut worker = WorkerBackend::spawn(&config.model_dir, device)
                .context("启动 GPU Final worker 失败")?;
            worker.handshake().context("GPU Final worker 握手失败")?;
            let handle = Arc::new(Mutex::new(worker));
            Ok((Box::new(WorkerFinal(Arc::clone(&handle))), Some(handle)))
        }
    }
}

struct WorkerFinal(FinalHandle);

impl FinalBackend for WorkerFinal {
    fn transcribe(
        &mut self,
        samples: &[f32],
    ) -> anyhow::Result<voime_core::asr::r#final::AsrOutput> {
        let mut worker = self
            .0
            .lock()
            .map_err(|_| anyhow::anyhow!("worker 锁已损坏"))?;
        let mut last_error = None;
        for attempt in 0..3 {
            match FinalBackend::transcribe(&mut *worker, samples) {
                Ok(output) => return Ok(output),
                Err(error) => {
                    last_error = Some(error);
                    if attempt < 2 {
                        std::thread::sleep(std::time::Duration::from_secs(1 << attempt));
                        worker.restart().context("重启 GPU worker 失败")?;
                    }
                }
            }
        }
        Err(last_error.unwrap_or_else(|| anyhow::anyhow!("GPU worker 推理失败")))
    }
}

impl Daemon {
    pub fn new(
        config: Config,
        final_runtime: FinalRuntime,
        models: ModelCatalog,
        worker: Option<FinalHandle>,
        audio: CaptureHandle,
        ipc: IpcServer,
    ) -> Self {
        let running_id = config.model_id.clone();
        let gpu_fallback = final_runtime.fallback;
        Self {
            config,
            state: State::default(),
            final_runtime,
            models,
            worker,
            audio,
            ipc,
            running_id,
            gpu_fallback,
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
        vlog!("daemon 主循环启动");
        while !self.shutdown {
            let pipeline_events = self
                .final_runtime
                .pipeline
                .as_ref()
                .expect("daemon 主循环期间 pipeline 必须存在")
                .events
                .clone();
            crossbeam_channel::select! {
                recv(self.ipc.events()) -> message => match message {
                    Ok(event) => self.on_server_event(event),
                    Err(_) => break,
                },
                recv(pipeline_events) -> message => match message {
                    Ok(event) => self.on_pipeline(event),
                    Err(_) => {
                        self.fatal = Some(anyhow::anyhow!("pipeline 事件通道已关闭"));
                        self.shutdown = true;
                    }
                },
                default(Duration::from_millis(100)) => {}
            }
        }

        vlog!("daemon 开始退出 fatal={:?}", self.fatal.is_some());
        self.ipc.broadcast(&ServerMessage::Bye {});
        self.ipc.shutdown();
        if let Some(worker) = self.worker.take() {
            if let Ok(mut worker) = worker.lock() {
                let _ = worker.shutdown();
            }
        }
        let pipeline_result = match self.final_runtime.pipeline.take() {
            Some(pipeline) => pipeline.shutdown(),
            None => Ok(()),
        };
        self.audio.stop();
        vlog!("daemon 退出完成 pipeline_ok={}", pipeline_result.is_ok());
        match self.fatal.take() {
            Some(error) => Err(error),
            None => pipeline_result.context("停止 pipeline 失败"),
        }
    }

    fn on_server_event(&mut self, event: ServerEvent) {
        match event {
            ServerEvent::Connected { client_id } => {
                vlog!("连接建立 id={client_id}");
            }
            ServerEvent::Message { client_id, message } => self.on_client(client_id, message),
            ServerEvent::BadMessage { client_id, message } => {
                vlog!("坏消息 id={client_id}：{message}");
                self.ipc.send_to(
                    client_id,
                    &ServerMessage::Error {
                        code: ErrorCode::InvalidMessage,
                        message,
                    },
                );
                self.ipc.disconnect(client_id);
            }
            ServerEvent::Disconnected { client_id, role } => {
                vlog!("客户端断开 id={client_id} role={role:?}");
                if role == Some(Role::Frontend) {
                    self.on_frontend_disconnected();
                }
            }
        }
    }

    fn on_frontend_disconnected(&mut self) {
        self.state.addon = FrontendState::Disconnected;
        self.session_active = false;
        if self.state.mode == Some(Mode::Continuous) {
            self.set_pipeline_mode(Mode::Ptt);
        }
        if self.current_segment.is_some() {
            if let Some(pipeline) = self.final_runtime.pipeline.as_ref() {
                let _ = pipeline.finish_segment();
            }
        }
        self.sync_audio();
        self.broadcast_snapshot();
    }

    fn on_client(&mut self, client_id: u64, message: ClientMessage) {
        match message {
            ClientMessage::Hello {
                role,
                version,
                config,
            } => {
                vlog!("Hello id={client_id} role={role:?} version={version}");
                if version != voime_ipc::PROTOCOL_VERSION {
                    self.ipc.send_to(
                        client_id,
                        &ServerMessage::Error {
                            code: ErrorCode::VersionMismatch,
                            message: format!(
                                "IPC 版本 {} 与 daemon {} 不一致",
                                version,
                                voime_ipc::PROTOCOL_VERSION
                            ),
                        },
                    );
                    self.ipc.disconnect(client_id);
                    return;
                }
                if !self.ipc.set_role(client_id, role) {
                    vlog!("角色冲突，断开 id={client_id}");
                    self.ipc.send_to(
                        client_id,
                        &ServerMessage::Error {
                            code: ErrorCode::RoleConflict,
                            message: "已有前端连接".to_string(),
                        },
                    );
                    self.ipc.disconnect(client_id);
                    return;
                }
                if role == Role::Frontend {
                    self.state.addon = FrontendState::Connected;
                    if let Some(config) = config {
                        self.state.apply_mode(config.mode);
                        self.state.hotkey = Some(config.hotkey);
                    }
                    self.apply_pipeline_mode();
                    if let Err(error) = self.sync_audio_result() {
                        self.send_error(
                            client_id,
                            ErrorCode::Internal,
                            &format!("打开录音设备失败：{error:#}"),
                        );
                    }
                    self.broadcast_snapshot();
                } else {
                    self.ipc.send_to(client_id, &self.state.snapshot());
                }
            }
            ClientMessage::Configure { mode, hotkey } => {
                vlog!("Configure mode={mode:?} hotkey={hotkey}");
                if self.ipc.role(client_id) != Some(Role::Frontend) {
                    self.send_error(client_id, ErrorCode::InvalidMessage, "只有前端可以配置");
                    return;
                }
                self.state.apply_mode(mode);
                self.state.hotkey = Some(hotkey);
                if mode != Mode::Toggle {
                    self.pending_stop = false;
                }
                if self.state.phase == Phase::Idle {
                    self.apply_pipeline_mode();
                    if mode != Mode::Continuous {
                        self.session_active = false;
                    }
                } else {
                    self.pending_mode = Some(mode);
                    vlog!("会话进行中，挂起模式 {}", mode.as_str());
                }
                if let Err(error) = self.sync_audio_result() {
                    self.send_error(
                        client_id,
                        ErrorCode::Internal,
                        &format!("切换录音设备状态失败：{error:#}"),
                    );
                }
                self.broadcast_snapshot();
            }
            ClientMessage::Start { segment_id, source } => {
                self.request_start(client_id, segment_id, source);
            }
            ClientMessage::Stop { segment_id, .. } => {
                self.request_stop(client_id, segment_id);
            }
            ClientMessage::Toggle { segment_id, source } => match self.state.phase {
                Phase::Idle => self.request_start(client_id, segment_id, source),
                Phase::Recording => self.request_stop(client_id, segment_id),
                Phase::Finalizing => self.send_error(client_id, ErrorCode::Busy, "正在识别"),
            },
            ClientMessage::ListModels {} => {
                let models = self.list_models();
                self.ipc
                    .send_to(client_id, &ServerMessage::Models { models });
            }
            ClientMessage::SetModel { package_id } => {
                if !self.can_manage_backend(client_id) {
                    return;
                }
                self.set_model(client_id, package_id);
            }
            ClientMessage::RestartBackend {} => {
                if !self.can_manage_backend(client_id) {
                    return;
                }
                self.restart_backend(client_id);
            }
            ClientMessage::ClearHistory {} => {
                self.state.clear_history();
                self.ipc.broadcast(&ServerMessage::HistoryCleared {});
            }
            ClientMessage::Status {} => {
                let recording = self.state.phase == Phase::Recording;
                let finalizing = self.state.phase == Phase::Finalizing;
                let status = self
                    .state
                    .status(recording, finalizing, self.current_segment);
                self.ipc
                    .send_to(client_id, &ServerMessage::Status { status });
            }
            ClientMessage::Quit {} => {
                if self.ipc.role(client_id) != Some(Role::Control) {
                    self.send_error(
                        client_id,
                        ErrorCode::InvalidMessage,
                        "只有 Control 可以退出",
                    );
                    return;
                }
                self.shutdown = true;
            }
        }
    }

    fn can_manage_backend(&self, client_id: u64) -> bool {
        match self.ipc.role(client_id) {
            Some(Role::Frontend | Role::Control) => true,
            _ => {
                self.send_error(
                    client_id,
                    ErrorCode::InvalidMessage,
                    "只有前端或 Control 可以管理模型",
                );
                false
            }
        }
    }

    fn list_models(&self) -> Vec<ModelInfo> {
        if self.config.explicit_final {
            return vec![self.current_model_info()];
        }
        let mut models = match ModelCatalog::scan(&self.config.model_root) {
            Ok(catalog) => catalog.list(),
            Err(error) => {
                vlog!("扫描模型目录失败：{error:#}");
                Vec::new()
            }
        };
        for model in &mut models {
            model.running = model.id == self.running_id;
        }
        if !models.iter().any(|model| model.id == self.running_id) {
            models.insert(0, self.current_model_info());
        }
        models
    }

    fn current_model_info(&self) -> ModelInfo {
        if self.gpu_fallback {
            return ModelInfo {
                id: voime_core::model::PREVIEW_PACKAGE_ID.to_string(),
                display_name: self.config.preview_manifest.display_name.clone(),
                kind: "sense-voice".to_string(),
                runtime: None,
                device: None,
                available: true,
                active: false,
                running: true,
            };
        }
        ModelInfo {
            id: self.config.model_id.clone(),
            display_name: self.config.manifest.display_name.clone(),
            kind: self.config.manifest.asr.kind.as_str().to_string(),
            runtime: self.config.manifest.backend.runtime.clone(),
            device: self.config.manifest.backend.device.clone(),
            available: true,
            active: self.config.explicit_final
                || config::read_active_id(&self.config.model_root)
                    .ok()
                    .flatten()
                    .as_deref()
                    == Some(self.config.model_id.as_str()),
            running: true,
        }
    }

    fn set_model(&mut self, client_id: u64, package_id: String) {
        if self.config.explicit_final {
            self.send_error(
                client_id,
                ErrorCode::InvalidMessage,
                "启动参数 --model-dir 固定了模型，不能通过界面切换",
            );
            return;
        }
        match ModelCatalog::scan(&self.config.model_root) {
            Ok(catalog) => {
                if let Some(entry) = catalog.find(&package_id) {
                    if !entry.available {
                        self.send_error(
                            client_id,
                            ErrorCode::Unavailable,
                            "GPU Final 不可用；请安装 runtime / 检查驱动与显存",
                        );
                        return;
                    }
                }
            }
            Err(error) => {
                self.send_error(
                    client_id,
                    ErrorCode::Internal,
                    &format!("扫描模型目录失败：{error:#}"),
                );
                return;
            }
        }
        match self.models.set_active(&package_id) {
            Ok(()) => {
                vlog!("模型选择已写入 active：{package_id}");
                self.ipc
                    .send_to(client_id, &ServerMessage::ModelSelected { package_id });
            }
            Err(error) => {
                self.send_error(
                    client_id,
                    ErrorCode::InvalidMessage,
                    &format!("无法选择模型包：{error:#}"),
                );
            }
        }
    }

    fn restart_backend(&mut self, client_id: u64) {
        if self.state.phase != Phase::Idle || self.current_segment.is_some() {
            self.send_error(client_id, ErrorCode::Busy, "录音或识别进行中，不能重启后端");
            return;
        }
        let old_model_id = self.config.model_id.clone();
        self.ipc.broadcast(&ServerMessage::BackendState {
            phase: BackendPhase::Restarting,
            model: None,
            message: None,
        });

        let new_config =
            match config::resolve_at(&self.config.model_root, self.final_runtime.args.clone()) {
                Ok(config) => config,
                Err(error) => {
                    self.rollback_active(&old_model_id);
                    self.backend_failed(client_id, format!("加载新模型包失败：{error:#}"));
                    return;
                }
            };
        let (backend, worker, punc) = match build_final(&new_config, &self.final_runtime.factory) {
            Ok((backend, worker)) => {
                let punc =
                    match pipeline::load_punc(&new_config.manifest, &self.final_runtime.factory) {
                        Ok(punc) => punc,
                        Err(error) => {
                            if let Some(worker) = worker.as_ref() {
                                if let Ok(mut worker) = worker.lock() {
                                    let _ = worker.shutdown();
                                }
                            }
                            self.rollback_active(&old_model_id);
                            self.backend_failed(client_id, format!("加载标点模型失败：{error:#}"));
                            return;
                        }
                    };
                (backend, worker, punc)
            }
            Err(error) => {
                self.rollback_active(&old_model_id);
                self.backend_failed(client_id, format!("启动新 Final 失败：{error:#}"));
                return;
            }
        };
        let update = FinalUpdate {
            backend,
            punc,
            punc_policy: new_config.manifest.asr.options.punc,
            max_segment_ms: new_config.manifest.max_segment_ms,
        };
        let Some(pipeline) = self.final_runtime.pipeline.as_ref() else {
            self.backend_failed(client_id, "pipeline 未运行".to_string());
            return;
        };
        if let Err(error) = pipeline.replace_final(update) {
            if let Some(worker) = worker.as_ref() {
                if let Ok(mut worker) = worker.lock() {
                    let _ = worker.shutdown();
                }
            }
            self.rollback_active(&old_model_id);
            self.backend_failed(client_id, format!("切换 Final 失败：{error:#}"));
            return;
        }

        let old_worker = std::mem::replace(&mut self.worker, worker);
        self.config = new_config;
        self.running_id = self.config.model_id.clone();
        self.gpu_fallback = false;
        if let Err(error) = self.models.refresh() {
            vlog!("刷新模型目录失败：{error:#}");
        }
        if let Some(old_worker) = old_worker {
            if let Ok(mut worker) = old_worker.lock() {
                let _ = worker.shutdown();
            }
        }
        self.state.phase = Phase::Idle;
        self.state.partial = None;
        self.current_segment = None;
        self.pending_stop = false;
        self.pending_mode = None;
        self.session_active = false;
        self.apply_pipeline_mode();
        self.sync_audio();
        vlog!("后端切换完成 -> {}", self.config.model_id);
        self.ipc.broadcast(&ServerMessage::BackendState {
            phase: BackendPhase::Ready,
            model: Some(self.current_model_info()),
            message: None,
        });
        self.broadcast_snapshot();
    }

    fn backend_failed(&self, client_id: u64, message: String) {
        vlog!("重启后端失败：{message}");
        self.ipc.broadcast(&ServerMessage::BackendState {
            phase: BackendPhase::Failed,
            model: Some(self.current_model_info()),
            message: Some(message.clone()),
        });
        self.send_error(client_id, ErrorCode::Internal, &message);
    }

    fn rollback_active(&mut self, old_model_id: &str) {
        if !self.config.explicit_final {
            if let Err(error) = config::write_active_id(&self.config.model_root, old_model_id) {
                vlog!("回滚 active 指针失败：{error:#}");
            }
        }
        if let Err(error) = self.models.refresh() {
            vlog!("回滚后刷新模型目录失败：{error:#}");
        }
    }

    fn request_start(&mut self, client_id: u64, segment_id: u64, source: TriggerSource) {
        if self.state.phase != Phase::Idle {
            vlog!(
                "Start 拒绝（Busy）id={client_id} phase={:?}",
                self.state.phase
            );
            self.send_error(client_id, ErrorCode::Busy, "已有语音段");
            return;
        }
        self.session_active = false;
        if let Err(error) = self.audio.play() {
            vlog!("Start 打开录音设备失败：{error:#}");
            self.send_error(
                client_id,
                ErrorCode::Internal,
                &format!("打开录音设备失败：{error:#}"),
            );
            return;
        }
        let _ = segment_id;
        self.pending_stop = false;
        let id = self.allocate_segment();
        self.current_segment = Some(id);
        self.state.phase = Phase::Recording;
        self.state.partial = None;
        let start = self
            .final_runtime
            .pipeline
            .as_ref()
            .context("pipeline 未运行")
            .and_then(Pipeline::start_segment);
        if let Err(error) = start {
            vlog!("Start 失败：{error:#}");
            self.current_segment = None;
            self.state.phase = Phase::Idle;
            self.sync_audio();
            self.send_error(client_id, ErrorCode::Internal, &format!("{error:#}"));
            return;
        }
        self.session_active = true;
        vlog!("语音段开始 id={id} source={source:?}");
        self.ipc.broadcast(&ServerMessage::SegmentStarted {
            segment_id: id,
            source,
        });
        self.broadcast_state();
    }

    fn request_stop(&mut self, client_id: u64, segment_id: u64) {
        if self.state.phase != Phase::Recording {
            // Toggle may stop between a Final and the continuation
            // SegmentStarted; remember it for the next Recording(true).
            vlog!(
                "Stop 落在 phase={:?}，pending_stop 锁存 id={client_id}",
                self.state.phase
            );
            if self.state.mode == Some(Mode::Toggle) {
                self.pending_stop = true;
            }
            if self.effective_mode() != Mode::Continuous {
                self.session_active = false;
            }
            self.sync_audio();
            return;
        }
        let Some(current) = self.current_segment else {
            return;
        };
        if segment_id != 0 && segment_id != current && self.state.mode != Some(Mode::Toggle) {
            vlog!("Stop 段不匹配 id={client_id} {segment_id} ≠ {current}");
            self.send_error(client_id, ErrorCode::Busy, "segment_id 不匹配");
            return;
        }
        if self.effective_mode() != Mode::Continuous {
            self.session_active = false;
        }
        vlog!("Stop seg={current} source 段收尾");
        if let Some(pipeline) = self.final_runtime.pipeline.as_ref() {
            if let Err(error) = pipeline.finish_segment() {
                vlog!("Stop 发送失败：{error:#}");
                self.send_error(client_id, ErrorCode::Internal, &format!("{error:#}"));
            }
        }
        self.sync_audio();
    }

    fn on_pipeline(&mut self, event: PipelineEvent) {
        match event {
            PipelineEvent::Recording(true) => {
                vlog!("pipeline Recording(true)");
                if self.current_segment.is_none() {
                    let id = self.allocate_segment();
                    self.current_segment = Some(id);
                    vlog!("续段开始 id={id}");
                    self.ipc.broadcast(&ServerMessage::SegmentStarted {
                        segment_id: id,
                        source: continuation_source(self.state.mode),
                    });
                }
                self.state.phase = Phase::Recording;
                self.state.partial = None;
                self.broadcast_state();
                if self.pending_stop {
                    self.pending_stop = false;
                    vlog!("pending_stop 命中，立即结束续段");
                    if let Some(pipeline) = self.final_runtime.pipeline.as_ref() {
                        let _ = pipeline.finish_segment();
                    }
                }
            }
            PipelineEvent::Recording(false) => {
                vlog!("pipeline Recording(false)");
                self.state.phase = Phase::Finalizing;
                self.broadcast_state();
            }
            PipelineEvent::Partial(text) => {
                if let Some(segment_id) = self.current_segment {
                    vlog!("Partial seg={segment_id} chars={}", text.chars().count());
                    self.state.partial = Some(text.clone());
                    self.ipc
                        .broadcast(&ServerMessage::Partial { segment_id, text });
                }
            }
            PipelineEvent::Final(text) => {
                vlog!("Final chars={}", text.chars().count());
                self.state.transcript.push(text.clone());
                self.state.partial = None;
                self.state.phase = Phase::Idle;
                if let Some(segment_id) = self.current_segment.take() {
                    self.ipc
                        .broadcast(&ServerMessage::Final { segment_id, text });
                }
                self.broadcast_state();
                self.apply_pending_mode();
            }
            PipelineEvent::SegmentDropped { reason } => {
                vlog!("语音段丢弃：{reason}");
                self.state.partial = None;
                self.state.phase = Phase::Idle;
                self.current_segment = None;
                self.ipc.broadcast(&ServerMessage::Error {
                    code: ErrorCode::Inference,
                    message: reason.clone(),
                });
                self.try_gpu_fallback(&reason);
                self.broadcast_state();
                self.apply_pending_mode();
            }
            PipelineEvent::Fatal(error) => {
                vlog!("pipeline 致命错误：{error}");
                eprintln!("voimed: pipeline 致命错误：{error}");
                self.fatal = Some(anyhow::anyhow!(error));
                self.shutdown = true;
            }
        }
    }

    /// GPU Final failure drops the current segment. The next segment runs on
    /// CPU SenseVoice without reloading Preview or restarting the daemon.
    fn try_gpu_fallback(&mut self, reason: &str) {
        if self.gpu_fallback || self.worker.is_none() {
            return;
        }
        if !reason.contains("ASR") && !reason.contains("worker") && !reason.contains("推理") {
            return;
        }
        let Some(worker) = self.worker.take() else {
            return;
        };
        if let Ok(mut worker) = worker.lock() {
            let _ = worker.shutdown();
        }
        let backend = match SenseVoiceBackend::new_with_factory(
            &self.config.preview_manifest.asr,
            &self.final_runtime.factory,
            pipeline::fbank_config(&self.config.preview_manifest.fbank),
        ) {
            Ok(backend) => backend,
            Err(error) => {
                vlog!("GPU 回退 CPU 失败：{error:#}");
                return;
            }
        };
        let update = FinalUpdate {
            backend: Box::new(backend),
            punc: None,
            punc_policy: voime_core::model::PuncPolicy::Builtin,
            max_segment_ms: self.config.preview_manifest.max_segment_ms,
        };
        let Some(pipeline) = self.final_runtime.pipeline.as_ref() else {
            return;
        };
        match pipeline.replace_final(update) {
            Ok(()) => {
                self.gpu_fallback = true;
                self.running_id = voime_core::model::PREVIEW_PACKAGE_ID.to_string();
                vlog!("GPU Final 不可用，已回退 CPU SenseVoice");
                self.ipc.broadcast(&ServerMessage::BackendState {
                    phase: BackendPhase::Failed,
                    model: Some(self.current_model_info()),
                    message: Some("GPU Final 不可用，已回退 CPU SenseVoice".to_string()),
                });
            }
            Err(error) => vlog!("切换 CPU Final 失败：{error:#}"),
        }
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

    fn set_pipeline_mode(&self, mode: Mode) {
        if let Some(pipeline) = self.final_runtime.pipeline.as_ref() {
            let _ = pipeline.set_mode(mode);
        }
    }

    fn apply_pipeline_mode(&self) {
        self.set_pipeline_mode(self.effective_mode());
    }

    fn apply_pending_mode(&mut self) {
        if let Some(mode) = self.pending_mode.take() {
            self.session_active = false;
            vlog!("应用挂起模式 {}", mode.as_str());
            self.set_pipeline_mode(if self.state.addon == FrontendState::Connected {
                mode
            } else {
                Mode::Ptt
            });
        }
        self.sync_audio();
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
            vlog!("切换录音流状态失败：{error:#}");
            eprintln!("voimed: 切换录音流状态失败：{error:#}");
        }
    }

    fn sync_audio_result(&mut self) -> anyhow::Result<()> {
        let should_play = self.audio_should_play();
        if should_play {
            self.audio.play()?;
        } else {
            self.audio.pause()?;
        }
        Ok(())
    }

    fn send_error(&self, client_id: u64, code: ErrorCode, message: &str) {
        self.ipc.send_to(
            client_id,
            &ServerMessage::Error {
                code,
                message: message.to_string(),
            },
        );
    }
}

/// Source used when the pipeline starts a segment without a matching
/// client-initiated Start.
fn continuation_source(mode: Option<Mode>) -> TriggerSource {
    match mode {
        Some(Mode::Ptt) => TriggerSource::Hold,
        Some(Mode::Toggle) => TriggerSource::Toggle,
        _ => TriggerSource::Continuous,
    }
}

fn listens(mode: Mode, session_active: bool) -> bool {
    mode == Mode::Continuous || session_active
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn audio_plays_only_for_continuous_or_active_session() {
        assert!(listens(Mode::Continuous, false));
        assert!(listens(Mode::Continuous, true));
        assert!(!listens(Mode::Ptt, false));
        assert!(listens(Mode::Ptt, true));
        assert!(!listens(Mode::Toggle, false));
        assert!(listens(Mode::Toggle, true));
    }

    #[test]
    fn toggle_continuation_keeps_toggle_source() {
        assert_eq!(
            continuation_source(Some(Mode::Toggle)),
            TriggerSource::Toggle
        );
        assert_eq!(continuation_source(Some(Mode::Ptt)), TriggerSource::Hold);
        assert_eq!(
            continuation_source(Some(Mode::Continuous)),
            TriggerSource::Continuous
        );
        assert_eq!(continuation_source(None), TriggerSource::Continuous);
    }
}
