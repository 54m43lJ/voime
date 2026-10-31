use voime_core::vad::firered::{VadFrame, FRAME_SHIFT_MS};
use voime_core::vad::segmenter::{
    SegmentEndReason, Segmenter, SegmenterEvent, MIN_SILENCE_MS, PTT_AUTO_END_MS,
};
use voime_ipc::Mode;

fn speech() -> VadFrame {
    VadFrame {
        speech_prob: 0.9,
        is_speech: true,
    }
}

fn silence() -> VadFrame {
    VadFrame {
        speech_prob: 0.1,
        is_speech: false,
    }
}

fn frame() -> Vec<f32> {
    vec![0.0; 160]
}

fn start(segmenter: &mut Segmenter, mode: Mode) {
    while !segmenter.is_active() {
        segmenter.push(speech(), &frame(), mode);
    }
}

#[test]
fn continuous_segment_ends_on_short_silence() {
    let mut segmenter = Segmenter::new(30_000);
    start(&mut segmenter, Mode::Continuous);
    let mut ended = false;
    for _ in 0..(MIN_SILENCE_MS / FRAME_SHIFT_MS) {
        if let Some(SegmenterEvent::SpeechEnd { reason, .. }) =
            segmenter.push(silence(), &frame(), Mode::Continuous)
        {
            assert_eq!(reason, SegmentEndReason::Silence);
            ended = true;
            break;
        }
    }
    assert!(ended);
    assert!(!segmenter.is_active());
}

#[test]
fn ptt_and_toggle_wait_for_longer_silence() {
    for mode in [Mode::Ptt, Mode::Toggle] {
        let mut segmenter = Segmenter::new(30_000);
        start(&mut segmenter, mode);
        for _ in 0..(MIN_SILENCE_MS / FRAME_SHIFT_MS) {
            assert!(segmenter.push(silence(), &frame(), mode).is_none());
        }
        let mut ended = false;
        for _ in 0..((PTT_AUTO_END_MS - MIN_SILENCE_MS) / FRAME_SHIFT_MS) {
            if matches!(
                segmenter.push(silence(), &frame(), mode),
                Some(SegmenterEvent::SpeechEnd { .. })
            ) {
                ended = true;
                break;
            }
        }
        assert!(ended, "mode {mode:?} should auto-end");
    }
}

#[test]
fn max_segment_ms_splits_long_speech() {
    let mut segmenter = Segmenter::new(500);
    start(&mut segmenter, Mode::Continuous);
    let mut ended = false;
    for _ in 0..100 {
        if let Some(SegmenterEvent::SpeechEnd { reason, .. }) =
            segmenter.push(speech(), &frame(), Mode::Continuous)
        {
            assert_eq!(reason, SegmentEndReason::MaxDuration);
            ended = true;
            break;
        }
    }
    assert!(ended);
}

#[test]
fn force_end_returns_active_segment_and_clears_state() {
    let mut segmenter = Segmenter::new(30_000);
    start(&mut segmenter, Mode::Ptt);
    let segment = segmenter.force_end().expect("active segment");
    assert!(!segment.samples.is_empty());
    assert!(segment.end_ms > segment.start_ms);
    assert!(!segmenter.is_active());
    assert!(segmenter.active_samples().is_empty());
}
