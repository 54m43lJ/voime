use anyhow::Result;
use kaldi_native_fbank::fbank::{FbankComputer, FbankOptions};
use kaldi_native_fbank::online::{FeatureComputer, OnlineFeature};
use kaldi_native_fbank::window::{extract_window, FrameOptions, Window};
use serde::Deserialize;

pub const NUM_BINS: usize = 80;
pub const FRAME_MS: u32 = 25;
pub const SHIFT_MS: u32 = 10;
/// The internal audio format is normalized to [-1, 1]; Kaldi fbank is trained
/// on the integer PCM scale, so features are computed from scaled samples.
pub const SAMPLE_SCALE: f32 = 32768.0;

/// Window function used by the fbank front end.
///
/// FireRed packages use `povey` (the Kaldi default); SenseVoice packages
/// declare `hamming`. Keeping this in the manifest makes the two front ends
/// explicit instead of relying on hidden defaults.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum WindowType {
    #[default]
    Povey,
    Hamming,
    Hanning,
    Hann,
    Sine,
    Rectangular,
    Blackman,
}

impl WindowType {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Povey => "povey",
            Self::Hamming => "hamming",
            Self::Hanning => "hanning",
            Self::Hann => "hann",
            Self::Sine => "sine",
            Self::Rectangular => "rectangular",
            Self::Blackman => "blackman",
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct FbankConfig {
    pub sample_rate: u32,
    pub num_bins: usize,
    pub frame_ms: u32,
    pub shift_ms: u32,
    pub window_type: WindowType,
}

impl Default for FbankConfig {
    fn default() -> Self {
        Self {
            sample_rate: crate::audio::resample::TARGET_SAMPLE_RATE,
            num_bins: NUM_BINS,
            frame_ms: FRAME_MS,
            shift_ms: SHIFT_MS,
            window_type: WindowType::Povey,
        }
    }
}

/// Computes the fbank frames for one complete audio segment.
pub struct Fbank {
    options: FbankOptions,
}

/// Sliding-window fbank for the streaming VAD. It keeps only one window of
/// PCM, so a continuous session cannot grow memory without bound.
pub struct StreamingFbank {
    computer: FbankComputer,
    window: Option<Window>,
    frame_options: FrameOptions,
    window_samples: usize,
    shift_samples: usize,
    pending: Vec<f32>,
}

impl StreamingFbank {
    pub fn new(config: FbankConfig) -> Self {
        let options = fbank_options(config);
        let computer = FbankComputer::new(options.clone()).expect("内置 fbank 配置必须有效");
        let window = Window::new(&options.frame_opts);
        let window_samples = ((config.sample_rate as u64 * config.frame_ms as u64) / 1000) as usize;
        let shift_samples = ((config.sample_rate as u64 * config.shift_ms as u64) / 1000) as usize;
        Self {
            computer,
            window,
            frame_options: options.frame_opts,
            window_samples: window_samples.max(1),
            shift_samples: shift_samples.max(1),
            pending: Vec::with_capacity(window_samples.max(1) * 2),
        }
    }

    pub fn push(&mut self, samples: &[f32]) -> Result<Vec<Vec<f32>>> {
        self.pending
            .extend(samples.iter().map(|sample| sample * SAMPLE_SCALE));
        let mut output = Vec::new();
        while self.pending.len() >= self.window_samples {
            output.push(self.compute_window());
            self.pending
                .drain(..self.shift_samples.min(self.pending.len()));
        }
        Ok(output)
    }

    fn compute_window(&mut self) -> Vec<f32> {
        let padded = self.frame_options.padded_window_size();
        let mut buffer = vec![0.0f32; padded];
        let raw_energy = extract_window(
            0,
            &self.pending[..self.window_samples],
            0,
            &self.frame_options,
            self.window.as_ref(),
            &mut buffer,
        )
        .expect("sliding fbank window is valid");
        let dimension = self.computer.dim();
        let mut feature = vec![0.0f32; dimension];
        self.computer
            .compute(raw_energy, 1.0, &mut buffer, &mut feature);
        feature
    }
}

impl Fbank {
    pub fn new(config: FbankConfig) -> Self {
        Self {
            options: fbank_options(config),
        }
    }

    /// Computes every complete frame of `samples` and flushes the final window.
    /// The returned frames include the whole segment, which is what CTC needs.
    pub fn compute(&mut self, samples: &[f32]) -> Result<Vec<Vec<f32>>> {
        if samples.is_empty() {
            return Ok(Vec::new());
        }
        let computer = FbankComputer::new(self.options.clone())
            .map_err(|error| anyhow::anyhow!("创建 fbank 计算器失败：{error}"))?;
        let mut online = OnlineFeature::new(FeatureComputer::Fbank(computer));
        let scaled: Vec<f32> = samples.iter().map(|sample| sample * SAMPLE_SCALE).collect();
        online.accept_waveform(crate::audio::resample::TARGET_SAMPLE_RATE as f32, &scaled);
        online.input_finished();
        let total = online.num_frames_ready();
        let mut frames = Vec::with_capacity(total);
        for index in 0..total {
            if let Some(frame) = online.get_frame(index) {
                frames.push(frame.to_vec());
            }
        }
        Ok(frames)
    }
}

fn fbank_options(config: FbankConfig) -> FbankOptions {
    let mut options = FbankOptions::default();
    options.frame_opts.samp_freq = config.sample_rate as f32;
    options.frame_opts.frame_length_ms = config.frame_ms as f32;
    options.frame_opts.frame_shift_ms = config.shift_ms as f32;
    options.frame_opts.dither = 0.0;
    options.frame_opts.snip_edges = true;
    options.frame_opts.window_type = config.window_type.as_str().to_string();
    options.mel_opts.num_bins = config.num_bins;
    options.mel_opts.low_freq = 20.0;
    options.mel_opts.high_freq = 0.0;
    options.use_energy = false;
    options.use_log_fbank = true;
    options.use_power = true;
    options.energy_floor = 0.0;
    options
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn produces_eighty_bins() {
        let mut fbank = Fbank::new(FbankConfig::default());
        let samples = vec![0.01f32; 16_000];
        let frames = fbank.compute(&samples).unwrap();
        assert!(!frames.is_empty());
        assert_eq!(frames[0].len(), NUM_BINS);
    }

    #[test]
    fn streaming_matches_expected_frame_count() {
        let mut fbank = StreamingFbank::new(FbankConfig::default());
        let frames = fbank.push(&vec![0.01f32; 16_000]).unwrap();
        // (16000 - 400) / 160 + 1 = 98 windows.
        assert_eq!(frames.len(), 98);
        assert_eq!(frames[0].len(), NUM_BINS);
    }

    #[test]
    fn empty_audio_has_no_frames() {
        let mut fbank = Fbank::new(FbankConfig::default());
        assert!(fbank.compute(&[]).unwrap().is_empty());
    }
}
