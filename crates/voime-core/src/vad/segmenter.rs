use voime_ipc::Mode;

use crate::model::MAX_SEGMENT_MS;
use crate::vad::firered::{VadFrame, FRAME_SHIFT_MS};

pub const TARGET_SAMPLE_RATE: u64 = 16_000;
pub const MIN_SPEECH_MS: u32 = 80;
pub const MIN_SILENCE_MS: u32 = 200;
pub const PTT_AUTO_END_MS: u32 = 1_200;

#[derive(Clone, Debug, PartialEq)]
pub struct Segment {
    pub samples: Vec<f32>,
    pub start_ms: u64,
    pub end_ms: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SegmentEndReason {
    Silence,
    MaxDuration,
}

#[derive(Clone, Debug, PartialEq)]
pub enum SegmenterEvent {
    SpeechStart,
    SpeechEnd {
        segment: Segment,
        reason: SegmentEndReason,
    },
}

pub struct Segmenter {
    max_segment_ms: u32,
    active: Option<Segment>,
    /// Frames accumulated while waiting for `MIN_SPEECH_MS` to be reached.
    pending: Vec<f32>,
    speech_run_ms: u32,
    silence_run_ms: u32,
    processed_ms: u64,
    speech_started_ms: u64,
}

impl Segmenter {
    pub fn new(max_segment_ms: u32) -> Self {
        Self {
            max_segment_ms: max_segment_ms.clamp(1, MAX_SEGMENT_MS),
            active: None,
            pending: Vec::new(),
            speech_run_ms: 0,
            silence_run_ms: 0,
            processed_ms: 0,
            speech_started_ms: 0,
        }
    }

    /// Feeds one VAD frame and its corresponding audio samples. Returns at most
    /// one event. `samples` is normally exactly one frame shift.
    pub fn push(&mut self, frame: VadFrame, samples: &[f32], mode: Mode) -> Option<SegmenterEvent> {
        let frame_ms = if samples.is_empty() {
            FRAME_SHIFT_MS
        } else {
            ((samples.len() as u64 * 1000) / TARGET_SAMPLE_RATE) as u32
        };
        let frame_start_ms = self.processed_ms;
        self.processed_ms += frame_ms as u64;

        if self.active.is_none() {
            if frame.is_speech {
                if self.speech_run_ms == 0 {
                    self.speech_started_ms = frame_start_ms;
                }
                self.speech_run_ms += frame_ms;
                self.pending.extend_from_slice(samples);
                if self.speech_run_ms >= MIN_SPEECH_MS {
                    let segment = Segment {
                        samples: std::mem::take(&mut self.pending),
                        start_ms: self.speech_started_ms,
                        end_ms: self.processed_ms,
                    };
                    self.active = Some(segment);
                    self.speech_run_ms = 0;
                    self.silence_run_ms = 0;
                    return Some(SegmenterEvent::SpeechStart);
                }
            } else {
                self.speech_run_ms = 0;
                self.pending.clear();
            }
            return None;
        }

        let segment = self.active.as_mut().expect("active checked");
        segment.samples.extend_from_slice(samples);
        segment.end_ms = self.processed_ms;

        if frame.is_speech {
            self.silence_run_ms = 0;
        } else {
            self.silence_run_ms += frame_ms;
        }

        let duration_ms = self.processed_ms.saturating_sub(self.speech_started_ms);
        let silence_limit = match mode {
            Mode::Continuous => MIN_SILENCE_MS,
            Mode::Ptt | Mode::Toggle => PTT_AUTO_END_MS,
        };

        let reason = if duration_ms >= self.max_segment_ms as u64 {
            Some(SegmentEndReason::MaxDuration)
        } else if self.silence_run_ms >= silence_limit {
            Some(SegmentEndReason::Silence)
        } else {
            None
        };
        if let Some(reason) = reason {
            let segment = self.active.take().expect("active checked");
            self.silence_run_ms = 0;
            return Some(SegmenterEvent::SpeechEnd { segment, reason });
        }
        None
    }

    pub fn force_end(&mut self) -> Option<Segment> {
        self.silence_run_ms = 0;
        self.pending.clear();
        self.speech_run_ms = 0;
        self.active.take()
    }

    pub fn is_active(&self) -> bool {
        self.active.is_some()
    }

    pub fn active_samples(&self) -> &[f32] {
        self.active
            .as_ref()
            .map(|segment| segment.samples.as_slice())
            .unwrap_or(&[])
    }

    pub fn reset(&mut self) {
        self.active = None;
        self.pending.clear();
        self.speech_run_ms = 0;
        self.silence_run_ms = 0;
        self.processed_ms = 0;
        self.speech_started_ms = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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

    #[test]
    fn emits_start_after_min_speech() {
        let mut segmenter = Segmenter::new(30_000);
        let samples = frame();
        assert!(segmenter
            .push(speech(), &samples, Mode::Continuous)
            .is_none());
        let mut started = false;
        for _ in 0..MIN_SPEECH_MS / FRAME_SHIFT_MS {
            if let Some(SegmenterEvent::SpeechStart) =
                segmenter.push(speech(), &samples, Mode::Continuous)
            {
                started = true;
                break;
            }
        }
        assert!(started);
        assert!(segmenter.is_active());
    }

    #[test]
    fn continuous_ends_after_min_silence() {
        let mut segmenter = Segmenter::new(30_000);
        let samples = frame();
        while !segmenter.is_active() {
            segmenter.push(speech(), &samples, Mode::Continuous);
        }
        let mut segment = None;
        for _ in 0..MIN_SILENCE_MS / FRAME_SHIFT_MS {
            if let Some(SegmenterEvent::SpeechEnd {
                segment: value,
                reason,
            }) = segmenter.push(silence(), &samples, Mode::Continuous)
            {
                assert_eq!(reason, SegmentEndReason::Silence);
                segment = Some(value);
                break;
            }
        }
        let segment = segment.expect("silence should end the segment");
        assert!(segment.end_ms > segment.start_ms);
    }

    #[test]
    fn ptt_and_toggle_use_longer_silence_threshold() {
        for mode in [Mode::Ptt, Mode::Toggle] {
            let mut segmenter = Segmenter::new(30_000);
            let samples = frame();
            while !segmenter.is_active() {
                segmenter.push(speech(), &samples, mode);
            }
            for _ in 0..MIN_SILENCE_MS / FRAME_SHIFT_MS {
                assert!(segmenter.push(silence(), &samples, mode).is_none());
            }
            let mut ended = false;
            for _ in 0..(PTT_AUTO_END_MS - MIN_SILENCE_MS) / FRAME_SHIFT_MS {
                if matches!(
                    segmenter.push(silence(), &samples, mode),
                    Some(SegmenterEvent::SpeechEnd { .. })
                ) {
                    ended = true;
                    break;
                }
            }
            assert!(ended);
        }
    }

    #[test]
    fn force_end_drops_short_pending_speech() {
        let mut segmenter = Segmenter::new(30_000);
        segmenter.push(speech(), &frame(), Mode::Ptt);
        assert!(segmenter.force_end().is_none());
    }

    #[test]
    fn max_segment_forces_end() {
        let mut segmenter = Segmenter::new(300);
        let samples = frame();
        while !segmenter.is_active() {
            segmenter.push(speech(), &samples, Mode::Continuous);
        }
        let mut ended = false;
        for _ in 0..100 {
            if let Some(SegmenterEvent::SpeechEnd { reason, .. }) =
                segmenter.push(speech(), &samples, Mode::Continuous)
            {
                assert_eq!(reason, SegmentEndReason::MaxDuration);
                ended = true;
                break;
            }
        }
        assert!(ended);
    }
}
