use std::fs::File;
use std::io::{BufReader, Read};
use std::path::{Component, Path, PathBuf};
use std::time::Instant;

use anyhow::{bail, Context};
use half::f16;
use kaldi_native_fbank::fbank::{FbankComputer, FbankOptions};
use kaldi_native_fbank::online::{FeatureComputer, OnlineFeature};
use ort::session::{Session, SessionInputValue};
use ort::value::{DynValue, Tensor, ValueType};
use serde::Deserialize;
use sha2::{Digest, Sha256};

use crate::runtime::GpuRuntime;

pub struct FireRedAed {
    encoder: Session,
    decoder: Session,
    tokens: Vec<String>,
    cmvn_mean: Vec<f32>,
    cmvn_inv_stddev: Vec<f32>,
    sos_id: i32,
    eos_id: i32,
    max_len: usize,
    fixed_cache_len: Option<usize>,
    num_decoder_layers: usize,
    num_head: usize,
    head_dim: usize,
    feature_bins: usize,
    encoder_x_kind: FloatKind,
    self_cache_kind: FloatKind,
    fbank: FbankOptions,
    encoder_x_name: String,
    encoder_len_name: String,
    encoder_k_name: String,
    encoder_v_name: String,
    tokens_name: String,
    self_k_name: String,
    self_v_name: String,
    cross_k_name: String,
    cross_v_name: String,
    offset_name: String,
    logits_name: String,
    out_self_k_name: String,
    out_self_v_name: String,
}

#[derive(Clone, Debug)]
pub struct AedOutput {
    pub text: String,
    pub latency_ms: u64,
    pub rtf: f32,
}

#[derive(Debug, Deserialize)]
struct Manifest {
    schema: u32,
    #[allow(dead_code)]
    display_name: String,
    #[serde(default)]
    requires: Vec<String>,
    fbank: FbankManifest,
    asr: AsrManifest,
}

#[derive(Debug, Deserialize)]
struct FbankManifest {
    num_bins: u32,
    frame_ms: u32,
    shift_ms: u32,
    window_type: String,
}

#[derive(Debug, Deserialize)]
struct AsrManifest {
    kind: String,
    encoder: ArtifactPath,
    decoder: ArtifactPath,
    precision: String,
    tokens: PathBuf,
    token_count: usize,
}

#[derive(Debug, Deserialize)]
struct ArtifactPath {
    model: PathBuf,
    sha256: String,
}

impl FireRedAed {
    pub fn load(manifest_dir: &Path, runtime: &GpuRuntime) -> anyhow::Result<Self> {
        let manifest_path = manifest_dir.join("manifest.json");
        let manifest: Manifest = serde_json::from_reader(BufReader::new(
            File::open(&manifest_path)
                .with_context(|| format!("无法读取 AED 清单 {}", manifest_path.display()))?,
        ))
        .with_context(|| format!("AED 清单格式错误 {}", manifest_path.display()))?;
        if manifest.schema != 3 {
            bail!("不支持的 AED schema：{}", manifest.schema);
        }
        if manifest.asr.kind != "fire-red-aed" {
            bail!("不是 fire-red-aed 包：{}", manifest.asr.kind);
        }
        if !manifest
            .requires
            .iter()
            .any(|id| id == "sense-voice-2024-int8")
        {
            bail!("FireRed AED 包必须 requires sense-voice-2024-int8");
        }
        if !matches!(
            manifest.asr.precision.to_ascii_lowercase().as_str(),
            "fp16" | "int8"
        ) {
            bail!(
                "FireRed AED 只接受 fp16 / int8（临时 v1）清单，实际 {}",
                manifest.asr.precision
            );
        }
        if manifest.fbank.window_type != "povey" {
            bail!("FireRed AED fbank 必须使用 povey 窗");
        }
        let encoder_path = resolve_inside(manifest_dir, &manifest.asr.encoder.model)?;
        let decoder_path = resolve_inside(manifest_dir, &manifest.asr.decoder.model)?;
        let tokens_path = resolve_inside(manifest_dir, &manifest.asr.tokens)?;
        verify_file(&encoder_path, &manifest.asr.encoder.sha256)?;
        verify_file(&decoder_path, &manifest.asr.decoder.sha256)?;
        let tokens = load_tokens(&tokens_path)?;
        if tokens.len() != manifest.asr.token_count {
            bail!(
                "AED 词表 {} 与清单 token_count {} 不一致",
                tokens.len(),
                manifest.asr.token_count
            );
        }

        let encoder = runtime.create(&encoder_path)?;
        let decoder = runtime.create(&decoder_path)?;
        let encoder_x = find_outlet(encoder.inputs(), &["x"], Some(3), true)
            .context("AED encoder 缺少 f32 三维输入 x")?;
        let encoder_len = find_outlet(
            encoder.inputs(),
            &["x_len", "x_lens", "length"],
            Some(1),
            false,
        )
        .context("AED encoder 缺少 i64 一维输入 x_len")?;
        let encoder_k = find_outlet(encoder.outputs(), &["cross_k"], Some(4), true)
            .context("AED encoder 缺少 f32 四维输出 cross_k")?;
        let encoder_v = find_outlet(encoder.outputs(), &["cross_v"], Some(4), true)
            .context("AED encoder 缺少 f32 四维输出 cross_v")?;

        let tokens_name = find_outlet(decoder.inputs(), &["tokens"], Some(2), false)
            .context("AED decoder 缺少 i64 二维输入 tokens")?;
        let self_k_name = find_outlet(decoder.inputs(), &["self_k"], Some(5), true)
            .context("AED decoder 缺少 f32 五维输入 self_k")?;
        // FireRedASR v1 hard-codes the self-attention cache length (1024) in
        // the decoder graph; v2 leaves it dynamic. Follow the graph shape.
        let fixed_cache_len = decoder
            .inputs()
            .iter()
            .find(|outlet| outlet.name() == self_k_name)
            .and_then(outlet_shape)
            .filter(|shape| shape.len() >= 3 && shape[2] > 0)
            .map(|shape| shape[2] as usize);
        let self_v_name = find_outlet(decoder.inputs(), &["self_v"], Some(5), true)
            .context("AED decoder 缺少 f32 五维输入 self_v")?;
        let cross_k_name = find_outlet(decoder.inputs(), &["cross_k"], Some(4), true)
            .context("AED decoder 缺少 f32 四维输入 cross_k")?;
        let cross_v_name = find_outlet(decoder.inputs(), &["cross_v"], Some(4), true)
            .context("AED decoder 缺少 f32 四维输入 cross_v")?;
        let offset_name = find_outlet(decoder.inputs(), &["offset"], Some(1), false)
            .context("AED decoder 缺少 i64 一维输入 offset")?;
        let logits_name = find_outlet(decoder.outputs(), &["logits"], Some(3), true)
            .context("AED decoder 缺少 f32 三维输出 logits")?;
        let out_self_k_name = find_outlet(decoder.outputs(), &["self_k"], Some(5), true)
            .context("AED decoder 缺少 f32 五维输出 self_k")?;
        let out_self_v_name = find_outlet(decoder.outputs(), &["self_v"], Some(5), true)
            .context("AED decoder 缺少 f32 五维输出 self_v")?;

        let metadata = encoder
            .metadata()
            .context("读取 AED encoder metadata 失败")?;
        let num_decoder_layers = read_usize(&metadata, "num_decoder_layers")?;
        let num_head = read_usize(&metadata, "num_head")?;
        let head_dim = read_usize(&metadata, "head_dim")?;
        let max_len = read_usize(&metadata, "max_len")?;
        let sos_id = read_i32(&metadata, "sos")?;
        let eos_id = read_i32(&metadata, "eos")?;
        if num_decoder_layers == 0 || num_head == 0 || head_dim == 0 || max_len == 0 {
            bail!("AED metadata 中的缓存维度必须大于 0");
        }
        if sos_id < 0
            || eos_id < 0
            || sos_id as usize >= tokens.len()
            || eos_id as usize >= tokens.len()
        {
            bail!("AED sos/eos token 非法：sos={sos_id} eos={eos_id}");
        }
        let cmvn_mean = metadata
            .custom("cmvn_mean")
            .context("AED encoder 缺少 cmvn_mean metadata")
            .and_then(|value| parse_cmvn(&value).context("解析 cmvn_mean 失败"))?;
        let cmvn_inv_stddev = metadata
            .custom("cmvn_inv_stddev")
            .context("AED encoder 缺少 cmvn_inv_stddev metadata")
            .and_then(|value| parse_cmvn(&value).context("解析 cmvn_inv_stddev 失败"))?;
        if cmvn_mean.is_empty() || cmvn_mean.len() != cmvn_inv_stddev.len() {
            bail!(
                "AED CMVN 维度非法：mean {} / inv_stddev {}",
                cmvn_mean.len(),
                cmvn_inv_stddev.len()
            );
        }
        let feature_bins = manifest.fbank.num_bins as usize;
        if feature_bins != cmvn_mean.len() {
            bail!(
                "AED fbank 维度 {} 与 CMVN 维度 {} 不一致",
                feature_bins,
                cmvn_mean.len()
            );
        }
        drop(metadata);

        let encoder_x_kind = input_float_kind(encoder.inputs(), &name_of(&encoder_x))?;
        let self_cache_kind = input_float_kind(decoder.inputs(), &name_of(&self_k_name))?;
        if input_float_kind(decoder.inputs(), &name_of(&self_v_name))? != self_cache_kind {
            bail!("AED self_k / self_v 的 dtype 不一致");
        }

        Ok(Self {
            encoder,
            decoder,
            tokens,
            cmvn_mean,
            cmvn_inv_stddev,
            sos_id,
            eos_id,
            max_len,
            fixed_cache_len,
            num_decoder_layers,
            num_head,
            head_dim,
            feature_bins,
            encoder_x_kind,
            self_cache_kind,
            fbank: fbank_options(&manifest.fbank),
            encoder_x_name: name_of(&encoder_x),
            encoder_len_name: name_of(&encoder_len),
            encoder_k_name: name_of(&encoder_k),
            encoder_v_name: name_of(&encoder_v),
            tokens_name: name_of(&tokens_name),
            self_k_name: name_of(&self_k_name),
            self_v_name: name_of(&self_v_name),
            cross_k_name: name_of(&cross_k_name),
            cross_v_name: name_of(&cross_v_name),
            offset_name: name_of(&offset_name),
            logits_name: name_of(&logits_name),
            out_self_k_name: name_of(&out_self_k_name),
            out_self_v_name: name_of(&out_self_v_name),
        })
    }

    pub fn transcribe(&mut self, samples: &[f32]) -> anyhow::Result<AedOutput> {
        let started = Instant::now();
        if samples.is_empty() {
            return Ok(AedOutput {
                text: String::new(),
                latency_ms: 0,
                rtf: 0.0,
            });
        }
        let features = compute_fbank(samples, &self.fbank)?;
        if features.is_empty() {
            return Ok(AedOutput {
                text: String::new(),
                latency_ms: 0,
                rtf: 0.0,
            });
        }
        let bins = self.feature_bins;
        let frames = features.len();
        let mut flat = Vec::with_capacity(frames * bins);
        for (index, frame) in features.iter().enumerate() {
            if frame.len() != bins {
                bail!(
                    "AED fbank 帧 {} 有 {} 个 bin，CMVN 期望 {}",
                    index,
                    frame.len(),
                    bins
                );
            }
            for (bin, value) in frame.iter().enumerate() {
                flat.push((*value - self.cmvn_mean[bin]) * self.cmvn_inv_stddev[bin]);
            }
        }

        let x = match self.encoder_x_kind {
            FloatKind::F32 => Tensor::from_array(([1usize, frames, bins], flat.into_boxed_slice()))
                .context("构造 AED encoder x 张量失败")?
                .into_dyn(),
            FloatKind::F16 => {
                let half: Vec<f16> = flat.iter().map(|value| f16::from_f32(*value)).collect();
                Tensor::from_array(([1usize, frames, bins], half.into_boxed_slice()))
                    .context("构造 FP16 AED encoder x 张量失败")?
                    .into_dyn()
            }
        };
        let x_len = Tensor::from_array(([1usize], vec![frames as i64].into_boxed_slice()))
            .context("构造 AED x_len 张量失败")?;
        let encoder_inputs: Vec<(String, SessionInputValue<'_>)> = vec![
            (self.encoder_x_name.clone(), x.into()),
            (self.encoder_len_name.clone(), x_len.into_dyn().into()),
        ];
        let mut encoder_outputs = self
            .encoder
            .run(encoder_inputs)
            .context("AED encoder 推理失败")?;
        let cross_k = encoder_outputs
            .remove(&self.encoder_k_name)
            .with_context(|| format!("AED encoder 输出缺失：{}", self.encoder_k_name))?;
        let cross_v = encoder_outputs
            .remove(&self.encoder_v_name)
            .with_context(|| format!("AED encoder 输出缺失：{}", self.encoder_v_name))?;
        drop(encoder_outputs);

        let cache_alloc_len = self.fixed_cache_len.unwrap_or(self.max_len);
        let cache_shape = [
            self.num_decoder_layers,
            1,
            cache_alloc_len,
            self.num_head,
            self.head_dim,
        ];
        let cache_len = cache_shape.iter().product::<usize>();
        let mut self_k = zero_cache(self.self_cache_kind, cache_shape, cache_len)
            .context("构造 AED self_k cache 失败")?;
        let mut self_v = zero_cache(self.self_cache_kind, cache_shape, cache_len)
            .context("构造 AED self_v cache 失败")?;

        let max_tokens = ((frames as f64 / 100.0 * 8.0) as usize)
            .max(1)
            .min((self.max_len / 2).max(1));
        let mut ids = Vec::new();
        let mut token = self.sos_id;
        for step in 0..max_tokens {
            let tokens =
                Tensor::from_array(([1usize, 1usize], vec![token as i64].into_boxed_slice()))
                    .context("构造 AED tokens 张量失败")?;
            let offset_tensor =
                Tensor::from_array(([1usize], vec![step as i64].into_boxed_slice()))
                    .context("构造 AED offset 张量失败")?;
            let inputs: Vec<(String, SessionInputValue<'_>)> = vec![
                (self.tokens_name.clone(), tokens.into_dyn().into()),
                (self.self_k_name.clone(), (&self_k).into()),
                (self.self_v_name.clone(), (&self_v).into()),
                (self.cross_k_name.clone(), (&cross_k).into()),
                (self.cross_v_name.clone(), (&cross_v).into()),
                (self.offset_name.clone(), offset_tensor.into_dyn().into()),
            ];
            let mut outputs = self.decoder.run(inputs).context("AED decoder 推理失败")?;
            let logits = outputs
                .remove(&self.logits_name)
                .with_context(|| format!("AED decoder 输出缺失：{}", self.logits_name))?;
            let (dims, logits) = logits_values(&logits).context("AED logits 输出类型错误")?;
            if dims.len() != 3 || dims[2] <= 0 {
                bail!("AED logits 形状非法：{:?}", dims);
            }
            let vocab = dims[2] as usize;
            if vocab != self.tokens.len() {
                bail!(
                    "AED logits 词表 {} 与 tokens {} 不一致",
                    vocab,
                    self.tokens.len()
                );
            }
            if logits.len() < vocab {
                bail!("AED logits 数据长度 {} 小于词表 {}", logits.len(), vocab);
            }
            let best = argmax(&logits[..vocab]);
            if best == self.eos_id as usize {
                break;
            }
            ids.push(best);
            token = best as i32;
            self_k = outputs
                .remove(&self.out_self_k_name)
                .with_context(|| format!("AED decoder 输出缺失：{}", self.out_self_k_name))?;
            self_v = outputs
                .remove(&self.out_self_v_name)
                .with_context(|| format!("AED decoder 输出缺失：{}", self.out_self_v_name))?;
        }
        let text = decode_tokens(&self.tokens, &ids);
        let elapsed = started.elapsed();
        let audio_seconds = samples.len() as f32 / 16_000.0;
        Ok(AedOutput {
            text,
            latency_ms: elapsed.as_millis() as u64,
            rtf: if audio_seconds > 0.0 {
                elapsed.as_secs_f32() / audio_seconds
            } else {
                0.0
            },
        })
    }
}

fn fbank_options(manifest: &FbankManifest) -> FbankOptions {
    let mut options = FbankOptions::default();
    options.frame_opts.samp_freq = 16_000.0;
    options.frame_opts.frame_length_ms = manifest.frame_ms as f32;
    options.frame_opts.frame_shift_ms = manifest.shift_ms as f32;
    options.frame_opts.dither = 0.0;
    options.frame_opts.snip_edges = true;
    options.frame_opts.window_type = "povey".to_string();
    options.mel_opts.num_bins = manifest.num_bins as usize;
    options.mel_opts.low_freq = 20.0;
    options.mel_opts.high_freq = 0.0;
    options.use_energy = false;
    options.use_log_fbank = true;
    options.use_power = true;
    options.energy_floor = 0.0;
    options
}

fn compute_fbank(samples: &[f32], options: &FbankOptions) -> anyhow::Result<Vec<Vec<f32>>> {
    let computer = FbankComputer::new(options.clone())
        .map_err(|error| anyhow::anyhow!("创建 AED fbank 计算器失败：{error}"))?;
    let mut online = OnlineFeature::new(FeatureComputer::Fbank(computer));
    let scaled: Vec<f32> = samples.iter().map(|sample| sample * 32768.0).collect();
    online.accept_waveform(16_000.0, &scaled);
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum FloatKind {
    F32,
    F16,
}

fn float_kind(dtype: &str) -> Option<FloatKind> {
    match dtype.to_ascii_lowercase().as_str() {
        "float32" | "f32" | "tensor(float)" => Some(FloatKind::F32),
        "float16" | "f16" | "tensor(float16)" => Some(FloatKind::F16),
        _ => None,
    }
}

fn outlet_shape(outlet: &ort::value::Outlet) -> Option<Vec<i64>> {
    let ValueType::Tensor { shape, .. } = outlet.dtype() else {
        return None;
    };
    Some(shape.to_vec())
}

fn input_float_kind(outlets: &[ort::value::Outlet], name: &str) -> anyhow::Result<FloatKind> {
    let outlet = outlets
        .iter()
        .find(|outlet| outlet.name() == name)
        .with_context(|| format!("AED 张量缺失：{name}"))?;
    let ValueType::Tensor { ty, .. } = outlet.dtype() else {
        bail!("AED 张量 {name} 不是普通张量");
    };
    float_kind(&format!("{ty:?}")).with_context(|| format!("AED 张量 {name} 不是 f32/f16"))
}

fn zero_cache(kind: FloatKind, shape: [usize; 5], len: usize) -> anyhow::Result<DynValue> {
    match kind {
        FloatKind::F32 => Tensor::from_array((shape, vec![0.0f32; len].into_boxed_slice()))
            .map(|tensor| tensor.into_dyn())
            .map_err(Into::into),
        FloatKind::F16 => Tensor::from_array((shape, vec![f16::ZERO; len].into_boxed_slice()))
            .map(|tensor| tensor.into_dyn())
            .map_err(Into::into),
    }
}

fn logits_values(value: &DynValue) -> anyhow::Result<(Vec<i64>, Vec<f32>)> {
    if let Ok((shape, data)) = value.try_extract_tensor::<f32>() {
        return Ok((shape.to_vec(), data.to_vec()));
    }
    let (shape, data) = value.try_extract_tensor::<f16>()?;
    Ok((
        shape.to_vec(),
        data.iter().map(|value| value.to_f32()).collect(),
    ))
}

fn find_outlet(
    outlets: &[ort::value::Outlet],
    needles: &[&str],
    rank: Option<usize>,
    want_f32: bool,
) -> Option<String> {
    for outlet in outlets {
        let ValueType::Tensor { ty, shape, .. } = outlet.dtype() else {
            continue;
        };
        if let Some(rank) = rank {
            if shape.len() != rank {
                continue;
            }
        }
        if want_f32 && float_kind(&format!("{ty:?}")).is_none() {
            continue;
        }
        if !want_f32 && !dtype_is_i64(&format!("{ty:?}")) {
            continue;
        }
        let name = outlet.name().to_ascii_lowercase();
        if needles.iter().any(|needle| name.contains(needle)) {
            return Some(outlet.name().to_string());
        }
    }
    // Fall back to the first outlet that matches the dtype and rank; real
    // exported graphs sometimes rename inputs (for example `x` -> `speech`).
    outlets.iter().find_map(|outlet| {
        let ValueType::Tensor { ty, shape, .. } = outlet.dtype() else {
            return None;
        };
        if rank.is_some_and(|rank| shape.len() != rank) {
            return None;
        }
        let dtype = format!("{ty:?}");
        if want_f32 && float_kind(&dtype).is_none() {
            return None;
        }
        if !want_f32 && !dtype_is_i64(&dtype) {
            return None;
        }
        Some(outlet.name().to_string())
    })
}

fn name_of(name: &str) -> String {
    name.to_owned()
}

fn dtype_is_i64(dtype: &str) -> bool {
    matches!(
        dtype.to_ascii_lowercase().as_str(),
        "int64" | "i64" | "tensor(int64)"
    )
}

fn read_usize(metadata: &ort::session::ModelMetadata<'_>, key: &str) -> anyhow::Result<usize> {
    metadata
        .custom(key)
        .with_context(|| format!("AED 模型缺少 {key} metadata"))?
        .trim()
        .parse::<usize>()
        .with_context(|| format!("解析 AED {key} 失败"))
}

fn read_i32(metadata: &ort::session::ModelMetadata<'_>, key: &str) -> anyhow::Result<i32> {
    metadata
        .custom(key)
        .with_context(|| format!("AED 模型缺少 {key} metadata"))?
        .trim()
        .parse::<i32>()
        .with_context(|| format!("解析 AED {key} 失败"))
}

fn parse_cmvn(text: &str) -> anyhow::Result<Vec<f32>> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Ok(Vec::new());
    }
    if trimmed.starts_with('[') {
        return serde_json::from_str(trimmed).context("解析 CMVN JSON 失败");
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

fn load_tokens(path: &Path) -> anyhow::Result<Vec<String>> {
    let content = std::fs::read_to_string(path)
        .with_context(|| format!("无法读取 AED 词表 {}", path.display()))?;
    let mut entries: Vec<Option<String>> = Vec::new();
    for (line_number, raw) in content.lines().enumerate() {
        let line = raw.trim_end_matches('\r');
        if line.is_empty() {
            continue;
        }
        let (token, id_text) = line.rsplit_once(' ').with_context(|| {
            format!(
                "词表行格式错误 {}:{}：缺少 id",
                path.display(),
                line_number + 1
            )
        })?;
        let id: usize = id_text.trim().parse().with_context(|| {
            format!(
                "词表行格式错误 {}:{}：id 非法 {}",
                path.display(),
                line_number + 1,
                id_text
            )
        })?;
        if entries.len() <= id {
            entries.resize(id + 1, None);
        }
        if entries[id].replace(token.to_string()).is_some() {
            bail!("词表 id 重复：{id}");
        }
    }
    if entries.is_empty() {
        bail!("词表为空：{}", path.display());
    }
    Ok(entries
        .into_iter()
        .map(|token| token.unwrap_or_default())
        .collect())
}

fn decode_tokens(tokens: &[String], ids: &[usize]) -> String {
    let mut text = String::new();
    for &id in ids {
        let Some(token) = tokens.get(id) else {
            continue;
        };
        if token.is_empty()
            || matches!(
                token.as_str(),
                "<blank>" | "<unk>" | "<sos>" | "<eos>" | "<pad>" | "<s>" | "</s>"
            )
            || (token.starts_with('<') && token.ends_with('>'))
            || (token.starts_with('[') && token.ends_with(']'))
        {
            continue;
        }
        if token == "<space>" {
            text.push(' ');
        } else {
            text.push_str(&token.replace('\u{2581}', " "));
        }
    }
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn argmax(values: &[f32]) -> usize {
    let mut best = 0usize;
    for (index, value) in values.iter().enumerate() {
        if *value > values[best] {
            best = index;
        }
    }
    best
}

fn resolve_inside(dir: &Path, relative: &Path) -> anyhow::Result<PathBuf> {
    if relative.as_os_str().is_empty() || relative.is_absolute() {
        bail!("AED 清单路径非法：{}", relative.display());
    }
    for component in relative.components() {
        if matches!(
            component,
            Component::ParentDir | Component::RootDir | Component::Prefix(_)
        ) {
            bail!("AED 清单路径非法：{}", relative.display());
        }
    }
    Ok(dir.join(relative))
}

fn verify_file(path: &Path, expected: &str) -> anyhow::Result<()> {
    let metadata =
        std::fs::metadata(path).with_context(|| format!("AED 模型文件缺失：{}", path.display()))?;
    if !metadata.is_file() || metadata.len() == 0 {
        bail!("AED 模型文件无效：{}", path.display());
    }
    let mut reader = BufReader::new(
        File::open(path).with_context(|| format!("无法读取 AED 模型 {}", path.display()))?,
    );
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let read = reader
            .read(&mut buffer)
            .with_context(|| format!("读取 AED 模型失败 {}", path.display()))?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    let actual = format!("{:x}", hasher.finalize());
    if !actual.eq_ignore_ascii_case(expected) {
        bail!(
            "AED 模型校验失败：{}（期望 {}，实际 {}）",
            path.display(),
            expected,
            actual
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn argmax_returns_first_highest_index() {
        assert_eq!(argmax(&[0.1, 0.9, 0.2]), 1);
        assert_eq!(argmax(&[1.0, 0.9, 0.2]), 0);
    }

    #[test]
    fn parses_cmvn_formats() {
        assert_eq!(parse_cmvn("[1.0, -2.0]").unwrap(), vec![1.0, -2.0]);
        assert_eq!(parse_cmvn("1.0, -2.0;3").unwrap(), vec![1.0, -2.0, 3.0]);
    }
}
