use voime_core::model::MAX_SEGMENT_MS;
use voime_core::vad::firered::{VadFrame, FRAME_SHIFT_MS};
use voime_core::vad::segmenter::{
    SegmentEndReason, Segmenter, SegmenterEvent, AUTO_END_SILENCE_MS,
};

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

fn frame_samples() -> Vec<f32> {
    vec![0.01; (16_000 * FRAME_SHIFT_MS as u64 / 1000) as usize]
}

#[test]
fn segment_starts_and_ends_on_vad() {
    let mut segmenter = Segmenter::new(MAX_SEGMENT_MS);
    let samples = frame_samples();
    while !segmenter.is_active() {
        let event = segmenter.push(speech(), &samples);
        if matches!(event, Some(SegmenterEvent::SpeechStart)) {
            break;
        }
    }
    assert!(segmenter.is_active());
    let mut ended = None;
    for _ in 0..AUTO_END_SILENCE_MS / FRAME_SHIFT_MS + 2 {
        if let Some(SegmenterEvent::SpeechEnd { segment, reason }) =
            segmenter.push(silence(), &samples)
        {
            ended = Some((segment, reason));
            break;
        }
    }
    let (segment, reason) = ended.expect("silence must end the segment");
    assert_eq!(reason, SegmentEndReason::Silence);
    assert!(!segment.samples.is_empty());
    assert!(segment.end_ms > segment.start_ms);
}

#[test]
fn max_duration_forces_end() {
    let mut segmenter = Segmenter::new(300);
    let samples = frame_samples();
    while !segmenter.is_active() {
        segmenter.push(speech(), &samples);
    }
    let mut reason = None;
    for _ in 0..100 {
        if let Some(SegmenterEvent::SpeechEnd { reason: ended, .. }) =
            segmenter.push(speech(), &samples)
        {
            reason = Some(ended);
            break;
        }
    }
    assert_eq!(reason, Some(SegmentEndReason::MaxDuration));
}

#[test]
fn force_end_drops_pending_short_speech() {
    let mut segmenter = Segmenter::new(MAX_SEGMENT_MS);
    segmenter.push(speech(), &frame_samples());
    assert!(segmenter.force_end().is_none());
}
