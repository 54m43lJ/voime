use anyhow::bail;
use kaldi_native_fbank::fbank::{FbankComputer, FbankOptions};
use kaldi_native_fbank::online::{FeatureComputer, OnlineFeature};

pub const NUM_BINS: usize = 80;
pub const FRAME_MS: u32 = 25;
pub const SHIFT_MS: u32 = 10;
pub const SAMPLE_SCALE: f32 = 32768.0;

#[derive(Clone, Copy, Debug)]
pub struct FbankConfig {
    pub sample_rate: u32,
    pub num_bins: usize,
    pub frame_ms: u32,
    pub shift_ms: u32,
    pub snip_edges: bool,
}

impl Default for FbankConfig {
    fn default() -> Self {
        Self {
            sample_rate: 16_000,
            num_bins: NUM_BINS,
            frame_ms: FRAME_MS,
            shift_ms: SHIFT_MS,
            snip_edges: true,
        }
    }
}

pub struct Fbank {
    options: FbankOptions,
    sample_rate: u32,
}

impl Fbank {
    pub fn new(config: FbankConfig) -> Self {
        Self {
            options: fbank_options(config),
            sample_rate: config.sample_rate,
        }
    }

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
        if frames.is_empty() {
            bail!("音频过短，无法计算 fbank");
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
    options.frame_opts.snip_edges = config.snip_edges;
    options.frame_opts.window_type = "povey".to_string();
    options.mel_opts.num_bins = config.num_bins;
    options.mel_opts.low_freq = 20.0;
    options.mel_opts.high_freq = 0.0;
    options.use_energy = false;
    options.use_log_fbank = true;
    options.use_power = true;
    options.energy_floor = 0.0;
    options
}
