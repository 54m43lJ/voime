//! 特征提取：kaldi 二进制 CMVN 解析 + kaldi-native-fbank 封装。
//!
//! 与 Python 端（kaldi_native_fbank / kaldiio）对齐的要点：
//! - waveform 为 i16 原始量纲的 f32（不做 /32768 归一化）
//! - FbankOptions: 80 mel、25ms/10ms、dither=0、snip_edges=true、povey 窗
//! - CMVN 为 kaldi 二进制 Matrix（"\0BDM "/"FM "，行 2 × 列 81）

use std::path::Path;

use kaldi_native_fbank::{
    fbank::{FbankComputer, FbankOptions},
    online::{FeatureComputer, OnlineFeature},
    window::{extract_window, Window},
};

use crate::error::{Result, VoimeError};

/// kaldi 风格 fbank 参数（对齐 FireRedASR2S 的 KaldifeatFbank）。
pub fn firered_fbank_opts() -> FbankOptions {
    let mut opts = FbankOptions::default();
    opts.frame_opts.samp_freq = 16000.0;
    opts.frame_opts.frame_length_ms = 25.0;
    opts.frame_opts.frame_shift_ms = 10.0;
    opts.frame_opts.dither = 0.0;
    opts.frame_opts.snip_edges = true;
    opts.mel_opts.num_bins = 80;
    opts.use_energy = false;
    opts.use_log_fbank = true;
    opts.use_power = true;
    opts.energy_floor = 0.0;
    opts
}

/// CMVN 统计（kaldi 二进制 Matrix，2×(dim+1)，最后一列为 count）。
pub struct Cmvn {
    pub means: Vec<f32>,
    pub istd: Vec<f32>,
}

impl Cmvn {
    pub fn load(path: &Path) -> Result<Self> {
        let data = std::fs::read(path)?;
        Self::parse(&data).map_err(|e| VoimeError::Model(format!("cmvn {}: {}", path.display(), e)))
    }

    /// 解析 "\0B" + token("FM "/"DM ") + size前缀的行列 + 数据。
    pub fn parse(data: &[u8]) -> std::result::Result<Self, String> {
        if data.len() < 5 || &data[0..2] != b"\0B" {
            return Err("bad magic".into());
        }
        let mut pos = 2;
        let mut token = String::new();
        while pos < data.len() && data[pos] != b' ' {
            token.push(data[pos] as char);
            pos += 1;
        }
        pos += 1; // skip space
        let read_i32 = |p: &mut usize| -> std::result::Result<i32, String> {
            let sz = data[*p] as usize;
            *p += 1;
            if sz != 4 || *p + 4 > data.len() {
                return Err(format!("bad int size {} at {}", sz, *p));
            }
            let v = i32::from_le_bytes(data[*p..*p + 4].try_into().unwrap());
            *p += 4;
            Ok(v)
        };
        let rows = read_i32(&mut pos)? as usize;
        let cols = read_i32(&mut pos)? as usize;
        if rows != 2 {
            return Err(format!("expect 2 rows, got {rows}"));
        }
        let n = rows * cols;
        let stats: Vec<f64> = match token.as_str() {
            "DM" => {
                if pos + n * 8 > data.len() {
                    return Err("truncated DM data".into());
                }
                (0..n)
                    .map(|i| f64::from_le_bytes(data[pos + i * 8..pos + i * 8 + 8].try_into().unwrap()))
                    .collect()
            }
            "FM" => {
                if pos + n * 4 > data.len() {
                    return Err("truncated FM data".into());
                }
                (0..n)
                    .map(|i| {
                        f32::from_le_bytes(data[pos + i * 4..pos + i * 4 + 4].try_into().unwrap())
                            as f64
                    })
                    .collect()
            }
            t => return Err(format!("unsupported token {t}")),
        };
        let dim = cols - 1;
        let count = stats[dim];
        if count < 1.0 {
            return Err("count < 1".into());
        }
        let mut means = Vec::with_capacity(dim);
        let mut istd = Vec::with_capacity(dim);
        for d in 0..dim {
            let mean = stats[d] / count;
            let var = stats[cols + d] / count - mean * mean;
            means.push(mean as f32);
            istd.push((1.0 / var.max(1e-20).sqrt()) as f32);
        }
        Ok(Self { means, istd })
    }

    pub fn dim(&self) -> usize {
        self.means.len()
    }

    /// 就地应用 (x - mean) * istd，feats 为行主序 [T, dim]。
    pub fn apply(&self, feats: &mut [f32]) {
        let dim = self.dim();
        debug_assert_eq!(feats.len() % dim, 0);
        for (i, x) in feats.iter_mut().enumerate() {
            let d = i % dim;
            *x = (*x - self.means[d]) * self.istd[d];
        }
    }
}

/// 整段音频的 fbank（对齐 Python：每次调用新建 OnlineFbank）。
///
/// 返回 (T, 80) 行主序特征（未做 CMVN）。
pub fn fbank_whole(opts: &FbankOptions, pcm: &[i16]) -> Result<Vec<Vec<f32>>> {
    let computer = FbankComputer::new(opts.clone()).map_err(VoimeError::Feature)?;
    let mut online = OnlineFeature::new(FeatureComputer::Fbank(computer));
    let wave: Vec<f32> = pcm.iter().map(|&s| s as f32).collect();
    online.accept_waveform(16000.0, &wave);
    Ok(online.features)
}

/// 单帧提取器（对齐 Python VAD：每个 400 样本块独立新建 OnlineFbank 取第 0 帧）。
///
/// 内部复用 FbankComputer/Window，避免逐帧重建。
pub struct SingleFrameFbank {
    opts: FbankOptions,
    computer: FbankComputer,
    window: Option<Window>,
}

impl SingleFrameFbank {
    pub fn new(opts: FbankOptions) -> Result<Self> {
        let computer = FbankComputer::new(opts.clone()).map_err(VoimeError::Feature)?;
        let window = Window::new(&opts.frame_opts);
        Ok(Self { opts, computer, window })
    }

    /// 输入 400 样本（25ms @16k），输出 80 维 fbank（未做 CMVN）。
    pub fn compute(&mut self, pcm400: &[i16]) -> Result<[f32; 80]> {
        if pcm400.len() != 400 {
            return Err(VoimeError::Feature(format!(
                "expect 400 samples, got {}",
                pcm400.len()
            )));
        }
        let wave: Vec<f32> = pcm400.iter().map(|&s| s as f32).collect();
        let padded = self.opts.frame_opts.padded_window_size();
        let mut buf = vec![0.0f32; padded];
        let raw_e = extract_window(0, &wave, 0, &self.opts.frame_opts, self.window.as_ref(), &mut buf)
            .map_err(|_| VoimeError::Feature("extract_window failed".into()))?;
        let dim = self.computer.dim();
        if dim != 80 {
            return Err(VoimeError::Feature(format!("expect dim 80, got {dim}")));
        }
        let mut feat = [0.0f32; 80];
        self.computer.compute(raw_e, 1.0, &mut buf, &mut feat);
        Ok(feat)
    }
}
