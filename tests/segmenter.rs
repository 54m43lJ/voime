use voime::cli::Mode;
use voime::vad::firered::VadFrame;
use voime::vad::segmenter::{SegmentEndReason, Segmenter, SegmenterConfig, SegmenterEvent};

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

fn samples() -> Vec<f32> {
    vec![0.0; 160]
}

#[test]
fn ptt_force_end_requires_minimum_speech() {
    let mut segmenter = Segmenter::new(SegmenterConfig::default());
    segmenter.push(speech(), &samples(), Mode::Ptt);
    assert!(segmenter.force_end().is_none());
}

#[test]
fn continuous_segment_ends_after_silence() {
    let config = SegmenterConfig::default();
    let mut segmenter = Segmenter::new(config);
    while !segmenter.is_active() {
        segmenter.push(speech(), &samples(), Mode::Continuous);
    }
    let mut ended = false;
    for _ in 0..config.min_silence_ms / 10 {
        if let Some(SegmenterEvent::SpeechEnd {
            segment: value,
            reason,
        }) = segmenter.push(silence(), &samples(), Mode::Continuous)
        {
            assert_eq!(reason, SegmentEndReason::Silence);
            assert!(value.end_ms > value.start_ms);
            ended = true;
            break;
        }
    }
    assert!(ended);
}

#[test]
fn segment_length_is_capped() {
    let config = SegmenterConfig {
        max_segment_ms: 250,
        ..SegmenterConfig::default()
    };
    let mut segmenter = Segmenter::new(config);
    while !segmenter.is_active() {
        segmenter.push(speech(), &samples(), Mode::Continuous);
    }
    let mut ended = false;
    for _ in 0..100 {
        if matches!(
            segmenter.push(speech(), &samples(), Mode::Continuous),
            Some(SegmenterEvent::SpeechEnd {
                reason: SegmentEndReason::MaxDuration,
                ..
            })
        ) {
            ended = true;
            break;
        }
    }
    assert!(ended);
}
