use std::time::Duration;

use anyhow::Context;
use voime_core::audio::capture::CaptureHandle;
use voime_core::config::Config;
use voime_core::pipeline::{Pipeline, PipelineEvent};
use voime_ipc::{
    ClientMessage, ErrorCode, FrontendState, Mode, Phase, Role, ServerMessage, TriggerSource,
};
use voime_log::vlog;

use crate::ipc_server::{IpcServer, ServerEvent};
use crate::state::State;

pub struct Daemon {
    _config: Config,
    state: State,
    pipeline: Pipeline,
    audio: CaptureHandle,
    ipc: IpcServer,
    current_segment: Option<u64>,
    next_segment: u64,
    pending_mode: Option<Mode>,
    pending_stop: bool,
    session_active: bool,
    shutdown: bool,
    fatal: Option<anyhow::Error>,
}

impl Daemon {
    pub fn new(config: Config, pipeline: Pipeline, audio: CaptureHandle, ipc: IpcServer) -> Self {
        Self {
            _config: config,
            state: State::default(),
            pipeline,
            audio,
            ipc,
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
            crossbeam_channel::select! {
                recv(self.ipc.events()) -> message => match message {
                    Ok(event) => self.on_server_event(event),
                    Err(_) => break,
                },
                recv(self.pipeline.events) -> message => match message {
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
        self.ipc.broadcast(&ServerMessage::Bye);
        self.ipc.shutdown();
        let pipeline_result = self.pipeline.shutdown();
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
            }
            ServerEvent::Disconnected { client_id, role } => {
                vlog!("客户端断开 id={client_id} role={role:?}");
                if role == Some(Role::Frontend) {
                    self.state.addon = FrontendState::Disconnected;
                    self.session_active = false;
                    if self.state.mode == Some(Mode::Continuous) {
                        let _ = self.pipeline.set_mode(Mode::Ptt);
                    }
                    if self.current_segment.is_some() {
                        let _ = self.pipeline.finish_segment();
                    }
                    self.sync_audio();
                    self.broadcast_snapshot();
                }
            }
        }
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
            ClientMessage::Stop {
                segment_id,
                source: _,
            } => {
                self.request_stop(client_id, segment_id);
            }
            ClientMessage::Toggle { segment_id, source } => match self.state.phase {
                Phase::Idle => self.request_start(client_id, segment_id, source),
                Phase::Recording => self.request_stop(client_id, segment_id),
                Phase::Finalizing => self.send_error(client_id, ErrorCode::Busy, "正在识别"),
            },
            ClientMessage::ClearHistory => {
                self.state.clear_history();
                self.ipc.broadcast(&ServerMessage::HistoryCleared);
            }
            ClientMessage::Status => {
                let recording = self.state.phase == Phase::Recording;
                let finalizing = self.state.phase == Phase::Finalizing;
                let status = self
                    .state
                    .status(recording, finalizing, self.current_segment);
                self.ipc
                    .send_to(client_id, &ServerMessage::Status { status });
            }
            ClientMessage::Quit => {
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
        if let Err(error) = self.pipeline.start_segment() {
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
        if let Err(error) = self.pipeline.finish_segment() {
            vlog!("Stop 发送失败：{error:#}");
            self.send_error(client_id, ErrorCode::Internal, &format!("{error:#}"));
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
                    let _ = self.pipeline.finish_segment();
                }
            }
            PipelineEvent::Recording(false) => {
                vlog!("pipeline Recording(false)");
                self.state.phase = Phase::Finalizing;
                self.broadcast_state();
            }
            PipelineEvent::Partial(text) => {
                if let Some(segment_id) = self.current_segment {
                    vlog!("Partial seg={segment_id} chars={}", text.len());
                    self.state.partial = Some(text.clone());
                    self.ipc
                        .broadcast(&ServerMessage::Partial { segment_id, text });
                }
            }
            PipelineEvent::Final(text) => {
                vlog!("Final chars={}", text.len());
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
                    message: reason,
                });
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

    fn allocate_segment(&mut self) -> u64 {
        let id = self.next_segment;
        self.next_segment = self.next_segment.wrapping_add(1).max(1);
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
        let mode = self.effective_mode();
        let _ = self.pipeline.set_mode(mode);
    }

    fn apply_pending_mode(&mut self) {
        if let Some(mode) = self.pending_mode.take() {
            self.session_active = false;
            vlog!("应用挂起模式 {}", mode.as_str());
            let _ = self
                .pipeline
                .set_mode(if self.state.addon == FrontendState::Connected {
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
        if self.audio_should_play() {
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
/// client-initiated Start. PTT max-duration splits keep Hold, toggle sessions
/// continue as Toggle, and continuous mode uses Continuous.
fn listens(mode: Mode, session_active: bool) -> bool {
    mode == Mode::Continuous || session_active
}

fn continuation_source(mode: Option<Mode>) -> TriggerSource {
    match mode {
        Some(Mode::Ptt) => TriggerSource::Hold,
        Some(Mode::Toggle) => TriggerSource::Toggle,
        _ => TriggerSource::Continuous,
    }
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
