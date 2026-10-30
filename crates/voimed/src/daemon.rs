use std::time::Duration;

use anyhow::Context;
use voime_core::audio::capture::CaptureHandle;
use voime_core::config::Config;
use voime_core::pipeline::{Pipeline, PipelineEvent};
use voime_ipc::{
    AddonState, ClientMessage, ErrorCode, Mode, Phase, Role, ServerMessage, TriggerSource,
};

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
            shutdown: false,
            fatal: None,
        }
    }

    pub fn run(mut self) -> anyhow::Result<()> {
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

        self.ipc.broadcast(&ServerMessage::Bye);
        self.ipc.shutdown();
        let pipeline_result = self.pipeline.shutdown();
        self.audio.stop();
        match self.fatal.take() {
            Some(error) => Err(error),
            None => pipeline_result.context("停止 pipeline 失败"),
        }
    }

    fn on_server_event(&mut self, event: ServerEvent) {
        match event {
            ServerEvent::Connected { client_id } => {
                let _ = client_id;
            }
            ServerEvent::Message { client_id, message } => self.on_client(client_id, message),
            ServerEvent::BadMessage { client_id, message } => {
                self.ipc.send_to(
                    client_id,
                    &ServerMessage::Error {
                        code: ErrorCode::InvalidMessage,
                        message,
                    },
                );
            }
            ServerEvent::Disconnected { client_id, role } => {
                let _ = client_id;
                if role == Some(Role::Fcitx) {
                    self.state.addon = AddonState::Disconnected;
                    if self.state.mode == Some(Mode::Continuous) {
                        let _ = self.pipeline.set_mode(Mode::Ptt);
                    }
                    if self.current_segment.is_some() {
                        let _ = self.pipeline.finish_segment();
                    }
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
                    self.ipc.send_to(
                        client_id,
                        &ServerMessage::Error {
                            code: ErrorCode::RoleConflict,
                            message: "已有 Fcitx 客户端连接".to_string(),
                        },
                    );
                    self.ipc.disconnect(client_id);
                    return;
                }
                if role == Role::Fcitx {
                    self.state.addon = AddonState::Connected;
                    if let Some(config) = config {
                        self.state.apply_mode(config.mode);
                        self.state.hotkey = Some(config.hotkey);
                    }
                    self.apply_pipeline_mode();
                    self.broadcast_snapshot();
                } else {
                    self.ipc.send_to(client_id, &self.state.snapshot());
                }
            }
            ClientMessage::Configure { mode, hotkey } => {
                if self.ipc.role(client_id) != Some(Role::Fcitx) {
                    self.send_error(client_id, ErrorCode::InvalidMessage, "只有 Fcitx 可以配置");
                    return;
                }
                self.state.apply_mode(mode);
                self.state.hotkey = Some(hotkey);
                if mode != Mode::Toggle {
                    self.pending_stop = false;
                }
                if self.state.phase == Phase::Idle {
                    self.apply_pipeline_mode();
                } else {
                    self.pending_mode = Some(mode);
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
            self.send_error(client_id, ErrorCode::Busy, "已有语音段");
            return;
        }
        let _ = segment_id;
        self.pending_stop = false;
        let id = self.allocate_segment();
        self.current_segment = Some(id);
        self.state.phase = Phase::Recording;
        self.state.partial = None;
        if let Err(error) = self.pipeline.start_segment() {
            self.current_segment = None;
            self.state.phase = Phase::Idle;
            self.send_error(client_id, ErrorCode::Internal, &format!("{error:#}"));
            return;
        }
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
            if self.state.mode == Some(Mode::Toggle) {
                self.pending_stop = true;
            }
            return;
        }
        let Some(current) = self.current_segment else {
            return;
        };
        if segment_id != 0 && segment_id != current && self.state.mode != Some(Mode::Toggle) {
            self.send_error(client_id, ErrorCode::Busy, "segment_id 不匹配");
            return;
        }
        if let Err(error) = self.pipeline.finish_segment() {
            self.send_error(client_id, ErrorCode::Internal, &format!("{error:#}"));
        }
    }

    fn on_pipeline(&mut self, event: PipelineEvent) {
        match event {
            PipelineEvent::Recording(true) => {
                if self.current_segment.is_none() {
                    let id = self.allocate_segment();
                    self.current_segment = Some(id);
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
                    let _ = self.pipeline.finish_segment();
                }
            }
            PipelineEvent::Recording(false) => {
                self.state.phase = Phase::Finalizing;
                self.broadcast_state();
            }
            PipelineEvent::Partial(text) => {
                if let Some(segment_id) = self.current_segment {
                    self.state.partial = Some(text.clone());
                    self.ipc
                        .broadcast(&ServerMessage::Partial { segment_id, text });
                }
            }
            PipelineEvent::Final(text) => {
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
            let _ = self
                .pipeline
                .set_mode(if self.state.addon == AddonState::Connected {
                    mode
                } else {
                    Mode::Ptt
                });
        }
    }

    fn effective_mode(&self) -> Mode {
        if self.state.addon == AddonState::Connected {
            self.state.mode.unwrap_or(Mode::Ptt)
        } else {
            Mode::Ptt
        }
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
