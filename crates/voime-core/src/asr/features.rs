use anyhow::{bail, Context};
use kaldi_native_fbank::fbank::{FbankComputer, FbankOptions};
use kaldi_native_fbank::online::{FeatureComputer, OnlineFeature};
use kaldi_native_fbank::window::{extract_window, FrameOptions, Window};

use crate::audio::resample::TARGET_SAMPLE_RATE;

pub const NUM_BINS: usize = 80;
pub const FRAME_MS: u32 = 25;
pub const SHIFT_MS: u32 = 10;
/// The internal audio format is normalized to [-1, 1]; Kaldi fbank is trained
/// on the integer PCM scale, so features are computed from scaled samples.
pub const SAMPLE_SCALE: f32 = 32768.0;
pub const LFR_WINDOW: usize = 7;
pub const LFR_SHIFT: usize = 6;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WindowType {
    Povey,
    Hamming,
}

impl WindowType {
    fn as_str(self) -> &'static str {
        match self {
            Self::Povey => "povey",
            Self::Hamming => "hamming",
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct FbankConfig {
    pub sample_rate: u32,
    pub num_bins: usize,
    pub frame_ms: u32,
    pub shift_ms: u32,
    pub window: WindowType,
}

impl Default for FbankConfig {
    fn default() -> Self {
        Self {
            sample_rate: TARGET_SAMPLE_RATE,
            num_bins: NUM_BINS,
            frame_ms: FRAME_MS,
            shift_ms: SHIFT_MS,
            // Kaldi 默认窗；FireRed VAD 与上游一致使用 povey。
            window: WindowType::Povey,
        }
    }
}

impl FbankConfig {
    /// SenseVoice 的上游导出使用 hamming 窗，不能与 FireRed VAD 共用默认值。
    pub fn sensevoice() -> Self {
        Self {
            window: WindowType::Hamming,
            ..Self::default()
        }
    }
}

/// Computes the fbank frames for one complete audio segment.
pub struct Fbank {
    options: FbankOptions,
    sample_rate: u32,
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

    pub fn push(&mut self, samples: &[f32]) -> anyhow::Result<Vec<Vec<f32>>> {
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
            sample_rate: config.sample_rate,
        }
    }

    /// Computes every complete frame of `samples` and flushes the final window.
    pub fn compute(&mut self, samples: &[f32]) -> anyhow::Result<Vec<Vec<f32>>> {
        if samples.is_empty() {
            return Ok(Vec::new());
        }
        let computer = FbankComputer::new(self.options.clone())
            .map_err(|error| anyhow::anyhow!("创建 fbank 计算器失败：{error}"))?;
        let mut online = OnlineFeature::new(FeatureComputer::Fbank(computer));
        let scaled: Vec<f32> = samples.iter().map(|sample| sample * SAMPLE_SCALE).collect();
        online.accept_waveform(self.sample_rate as f32, &scaled);
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

pub fn apply_lfr(features: &[Vec<f32>], window: usize, shift: usize) -> Vec<Vec<f32>> {
    if features.is_empty() || window == 0 || shift == 0 || features.len() < window {
        return Vec::new();
    }
    let bins = features[0].len();
    let output_frames = (features.len() - window) / shift + 1;
    let mut output = Vec::with_capacity(output_frames);
    for index in 0..output_frames {
        let start = index * shift;
        let mut frame = Vec::with_capacity(bins * window);
        for source in features.iter().skip(start).take(window) {
            frame.extend_from_slice(source);
        }
        output.push(frame);
    }
    output
}

pub fn apply_cmvn(
    features: &mut [Vec<f32>],
    mean: &[f32],
    inv_stddev: &[f32],
) -> anyhow::Result<()> {
    if mean.len() != inv_stddev.len() {
        bail!(
            "CMVN mean {} 与 inv_stddev {} 长度不一致",
            mean.len(),
            inv_stddev.len()
        );
    }
    for (index, frame) in features.iter_mut().enumerate() {
        if frame.len() != mean.len() {
            bail!(
                "CMVN 帧 {} 维度 {}，期望 {}",
                index,
                frame.len(),
                mean.len()
            );
        }
        for (bin, value) in frame.iter_mut().enumerate() {
            *value = (*value - mean[bin]) * inv_stddev[bin];
        }
    }
    Ok(())
}

fn fbank_options(config: FbankConfig) -> FbankOptions {
    let mut options = FbankOptions::default();
    options.frame_opts.samp_freq = config.sample_rate as f32;
    options.frame_opts.frame_length_ms = config.frame_ms as f32;
    options.frame_opts.frame_shift_ms = config.shift_ms as f32;
    options.frame_opts.dither = 0.0;
    options.frame_opts.snip_edges = true;
    options.frame_opts.window_type = config.window.as_str().to_string();
    options.mel_opts.num_bins = config.num_bins;
    options.mel_opts.low_freq = 20.0;
    options.mel_opts.high_freq = 0.0;
    options.use_energy = false;
    options.use_log_fbank = true;
    options.use_power = true;
    options.energy_floor = 0.0;
    options
}

/// Parses CMVN metadata emitted by sherpa-onnx. Accepts a JSON array or a
/// whitespace / comma / semicolon separated float string.
pub fn parse_cmvn(text: &str) -> anyhow::Result<Vec<f32>> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Ok(Vec::new());
    }
    if trimmed.starts_with('[') {
        return serde_json::from_str(trimmed).context("解析 CMVN JSON 数组失败");
    }
    let mut values = Vec::new();
    for part in trimmed.split([' ', ',', ';', '\n', '\t']) {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        values.push(
            part.parse::<f32>()
                .with_context(|| format!("解析 CMVN 数值失败：{part}"))?,
        );
    }
    Ok(values)
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
        assert_eq!(frames.len(), 98);
        assert_eq!(frames[0].len(), NUM_BINS);
    }

    #[test]
    fn vad_and_sensevoice_use_different_windows() {
        assert_eq!(
            fbank_options(FbankConfig::default()).frame_opts.window_type,
            "povey"
        );
        assert_eq!(
            fbank_options(FbankConfig::sensevoice())
                .frame_opts
                .window_type,
            "hamming"
        );
    }

    #[test]
    fn lfr_uses_7_by_6_and_emits_complete_windows() {
        let features: Vec<Vec<f32>> = (0..13).map(|value| vec![value as f32]).collect();
        let output = apply_lfr(&features, LFR_WINDOW, LFR_SHIFT);
        assert_eq!(output.len(), 2);
        assert_eq!(output[0], vec![0.0, 1.0, 2.0, 3.0, 4.0, 5.0, 6.0]);
        assert_eq!(output[1], vec![6.0, 7.0, 8.0, 9.0, 10.0, 11.0, 12.0]);
    }

    #[test]
    fn cmvn_subtracts_mean() {
        let mut frames = vec![vec![1.0, 2.0], vec![3.0, 4.0]];
        apply_cmvn(&mut frames, &[1.0, 0.5], &[2.0, 4.0]).unwrap();
        assert_eq!(frames, vec![vec![0.0, 6.0], vec![4.0, 14.0]]);
    }

    #[test]
    fn empty_audio_has_no_frames() {
        let mut fbank = Fbank::new(FbankConfig::default());
        assert!(fbank.compute(&[]).unwrap().is_empty());
    }
}
