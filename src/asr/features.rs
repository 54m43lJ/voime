use anyhow::Result;
use kaldi_native_fbank::fbank::{FbankComputer, FbankOptions};
use kaldi_native_fbank::online::{FeatureComputer, OnlineFeature};
use kaldi_native_fbank::window::{extract_window, FrameOptions, Window};

pub const NUM_BINS: usize = 80;
pub const FRAME_MS: u32 = 25;
pub const SHIFT_MS: u32 = 10;
/// The internal audio format is normalized to [-1, 1]; Kaldi fbank is trained
/// on the integer PCM scale, so features are computed from scaled samples.
pub const SAMPLE_SCALE: f32 = 32768.0;

#[derive(Clone, Copy, Debug)]
pub struct FbankConfig {
    pub sample_rate: u32,
    pub num_bins: usize,
    pub frame_ms: u32,
    pub shift_ms: u32,
}

impl Default for FbankConfig {
    fn default() -> Self {
        Self {
            sample_rate: crate::audio::resample::TARGET_SAMPLE_RATE,
            num_bins: NUM_BINS,
            frame_ms: FRAME_MS,
            shift_ms: SHIFT_MS,
        }
    }
}

pub struct Fbank {
    online: OnlineFeature,
    frames_read: usize,
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
        let options = fbank_options(config);
        let computer = FbankComputer::new(options).expect("内置 fbank 配置必须有效");
        Self {
            online: OnlineFeature::new(FeatureComputer::Fbank(computer)),
            frames_read: 0,
        }
    }

    pub fn compute(&mut self, samples: &[f32]) -> Result<Vec<Vec<f32>>> {
        if samples.is_empty() {
            return Ok(Vec::new());
        }
        let scaled: Vec<f32> = samples.iter().map(|sample| sample * SAMPLE_SCALE).collect();
        self.online
            .accept_waveform(crate::audio::resample::TARGET_SAMPLE_RATE as f32, &scaled);
        Ok(self.take_ready())
    }

    /// Flushes the final incomplete window. Used once for a finished segment.
    pub fn finalize(&mut self) -> Vec<Vec<f32>> {
        self.online.input_finished();
        self.take_ready()
    }

    fn take_ready(&mut self) -> Vec<Vec<f32>> {
        let total = self.online.num_frames_ready();
        let mut frames = Vec::with_capacity(total.saturating_sub(self.frames_read));
        for index in self.frames_read..total {
            if let Some(frame) = self.online.get_frame(index) {
                frames.push(frame.to_vec());
            }
        }
        self.frames_read = total;
        frames
    }
}

fn fbank_options(config: FbankConfig) -> FbankOptions {
    let mut options = FbankOptions::default();
    options.frame_opts.samp_freq = config.sample_rate as f32;
    options.frame_opts.frame_length_ms = config.frame_ms as f32;
    options.frame_opts.frame_shift_ms = config.shift_ms as f32;
    options.frame_opts.dither = 0.0;
    options.frame_opts.snip_edges = true;
    options.mel_opts.num_bins = config.num_bins;
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
}
