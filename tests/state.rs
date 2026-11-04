use voime_ipc::{FrontendState, Mode, Phase, ServerMessage};
use voimed::state::State;

#[test]
fn default_state_is_idle_and_disconnected() {
    let state = State::default();
    assert_eq!(state.phase, Phase::Idle);
    assert_eq!(state.addon, FrontendState::Disconnected);
    assert!(state.transcript.is_empty());
}

#[test]
fn snapshot_restores_history_partial_and_config() {
    let mut state = State::default();
    state.apply_mode(Mode::Toggle);
    state.hotkey = Some("Ctrl+Alt+Space".to_string());
    state.phase = Phase::Recording;
    state.partial = Some("临时文本".to_string());
    state.transcript.push("最终文本".to_string());
    let ServerMessage::Snapshot {
        mode,
        phase,
        partial,
        transcript,
        hotkey,
        ..
    } = state.snapshot()
    else {
        panic!("expected snapshot");
    };
    assert_eq!(mode, Some(Mode::Toggle));
    assert_eq!(phase, Phase::Recording);
    assert_eq!(partial.as_deref(), Some("临时文本"));
    assert_eq!(transcript, vec!["最终文本".to_string()]);
    assert_eq!(hotkey.as_deref(), Some("Ctrl+Alt+Space"));
}

#[test]
fn clear_history_keeps_partial() {
    let mut state = State::default();
    state.transcript.push("a".to_string());
    state.partial = Some("b".to_string());
    state.clear_history();
    assert!(state.transcript.is_empty());
    assert_eq!(state.partial.as_deref(), Some("b"));
}
