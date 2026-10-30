use voime_ipc::{AddonState, Mode, Phase, ServerMessage};
use voimed::state::State;

#[test]
fn snapshot_defaults_are_idle_and_empty() {
    let state = State::default();
    match state.snapshot() {
        ServerMessage::Snapshot {
            mode,
            phase,
            partial,
            transcript,
            addon,
            hotkey,
        } => {
            assert_eq!(mode, None);
            assert_eq!(phase, Phase::Idle);
            assert_eq!(partial, None);
            assert!(transcript.is_empty());
            assert_eq!(addon, AddonState::Disconnected);
            assert_eq!(hotkey, None);
        }
        other => panic!("unexpected snapshot: {other:?}"),
    }
}

#[test]
fn clear_history_keeps_current_segment_and_partial() {
    let mut state = State::default();
    state.transcript.push("第一句".to_string());
    state.transcript.push("第二句".to_string());
    state.partial = Some("正在识别".to_string());
    state.phase = Phase::Recording;
    state.apply_mode(Mode::Continuous);
    state.clear_history();
    assert!(state.transcript.is_empty());
    assert_eq!(state.partial.as_deref(), Some("正在识别"));
    assert_eq!(state.phase, Phase::Recording);
    assert_eq!(state.mode, Some(Mode::Continuous));
}

#[test]
fn status_reports_addon_mode_hotkey_and_history_count() {
    let mut state = State::default();
    state.apply_mode(Mode::Toggle);
    state.hotkey = Some("Super+F2".to_string());
    state.transcript.push("历史".to_string());
    state.addon = AddonState::Connected;
    let status = state.status(true, false, Some(9));
    assert!(status.recording);
    assert!(!status.finalizing);
    assert_eq!(status.segment_id, Some(9));
    assert_eq!(status.mode, Some(Mode::Toggle));
    assert_eq!(status.hotkey.as_deref(), Some("Super+F2"));
    assert_eq!(status.addon, AddonState::Connected);
    assert_eq!(status.transcript_count, 1);
}
