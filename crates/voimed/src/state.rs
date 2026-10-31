use voime_ipc::{FrontendState, Mode, Phase, ServerMessage, Status};

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
    fn clear_history_keeps_partial_and_phase() {
        let mut state = State::default();
        state.transcript.push("第一句".to_string());
        state.partial = Some("临时".to_string());
        state.phase = Phase::Recording;
        state.clear_history();
        assert!(state.transcript.is_empty());
        assert_eq!(state.partial.as_deref(), Some("临时"));
        assert_eq!(state.phase, Phase::Recording);
    }

    #[test]
    fn snapshot_carries_mode_and_addon_state() {
        let mut state = State::default();
        state.apply_mode(Mode::Continuous);
        state.hotkey = Some("Super+F2".to_string());
        state.addon = FrontendState::Connected;
        match state.snapshot() {
            ServerMessage::Snapshot {
                mode,
                addon,
                hotkey,
                ..
            } => {
                assert_eq!(mode, Some(Mode::Continuous));
                assert_eq!(addon, FrontendState::Connected);
                assert_eq!(hotkey.as_deref(), Some("Super+F2"));
            }
            other => panic!("unexpected snapshot: {other:?}"),
        }
    }
}
