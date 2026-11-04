use voime_ipc::{FrontendState, Mode, Phase, ServerMessage, Status};

#[derive(Clone, Debug)]
pub struct State {
    pub mode: Option<Mode>,
    pub hotkey: Option<String>,
    pub phase: Phase,
    pub partial: Option<String>,
    pub transcript: Vec<String>,
    pub addon: FrontendState,
}

impl Default for State {
    fn default() -> Self {
        Self {
            mode: None,
            hotkey: None,
            phase: Phase::Idle,
            partial: None,
            transcript: Vec::new(),
            addon: FrontendState::Disconnected,
        }
    }
}

impl State {
    pub fn apply_mode(&mut self, mode: Mode) {
        self.mode = Some(mode);
    }

    pub fn clear_history(&mut self) {
        self.transcript.clear();
    }

    pub fn snapshot(&self) -> ServerMessage {
        ServerMessage::Snapshot {
            mode: self.mode,
            phase: self.phase,
            partial: self.partial.clone(),
            transcript: self.transcript.clone(),
            addon: self.addon,
            hotkey: self.hotkey.clone(),
        }
    }

    pub fn status(&self, recording: bool, finalizing: bool, segment_id: Option<u64>) -> Status {
        Status {
            recording,
            finalizing,
            segment_id,
            mode: self.mode,
            hotkey: self.hotkey.clone(),
            addon: self.addon,
            transcript_count: self.transcript.len(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshot_carries_state_without_backend() {
        let mut state = State::default();
        state.apply_mode(Mode::Toggle);
        state.hotkey = Some("Ctrl+Alt+Space".to_string());
        state.transcript.push("你好".to_string());
        let snapshot = state.snapshot();
        let ServerMessage::Snapshot {
            mode,
            transcript,
            hotkey,
            ..
        } = snapshot
        else {
            panic!("expected snapshot");
        };
        assert_eq!(mode, Some(Mode::Toggle));
        assert_eq!(hotkey.as_deref(), Some("Ctrl+Alt+Space"));
        assert_eq!(transcript, vec!["你好".to_string()]);
    }

    #[test]
    fn status_reports_transcript_count() {
        let mut state = State::default();
        state.transcript.push("a".to_string());
        let status = state.status(true, false, Some(3));
        assert_eq!(status.transcript_count, 1);
        assert_eq!(status.segment_id, Some(3));
    }
}
