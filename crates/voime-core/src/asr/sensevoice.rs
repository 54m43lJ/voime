use anyhow::{bail, Context};
use ort::session::Session;
use ort::session::SessionInputValue;
use ort::value::Tensor;

use crate::asr::decode::SenseVoiceTokenizer;
use crate::asr::features::{
    apply_cmvn, apply_lfr, parse_cmvn, Fbank, FbankConfig, LFR_SHIFT, LFR_WINDOW,
};
use crate::infer::session::{dtype_is_f32, dtype_is_i32, SessionFactory};
use crate::model::ModelManifest;

/// SenseVoice prepends four CTC tokens: language, emotion, event and text
/// normalization. They are metadata, not transcript text.
pub const SENSE_VOICE_PREFIX_TOKENS: usize = 4;

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct AsrOutput {
    pub ids: Vec<usize>,
    pub text: String,
}

pub struct SenseVoice {
    session: Session,
    tokens: SenseVoiceTokenizer,
    mean: Vec<f32>,
    inv_stddev: Vec<f32>,
    lfr_window: usize,
    lfr_shift: usize,
    language_id: i32,
    with_itn_id: i32,
    without_itn_id: i32,
    use_itn: bool,
    x_name: String,
    x_length_name: String,
    language_name: String,
    text_norm_name: String,
    logits_name: String,
    token_count: usize,
    fbank: FbankConfig,
}

impl SenseVoice {
    pub fn new(
        manifest: &ModelManifest,
        factory: &SessionFactory,
        fbank: FbankConfig,
    ) -> anyhow::Result<Self> {
        let session = factory.build(manifest)?;
        let inputs = crate::infer::session::input_specs(&session)?;
        let outputs = crate::infer::session::output_specs(&session)?;

        let x = inputs
            .iter()
            .find(|spec| {
                dtype_is_f32(&spec.dtype)
                    && spec.shape.len() == 3
                    && spec.shape.last().copied().unwrap_or(0) > 0
            })
            .context("SenseVoice 模型缺少 [1, T, 560] 的 f32 输入 x")?;
        let x_length = find_input(&inputs, "x_length", "int32")?;
        let language = find_input(&inputs, "language", "int32")?;
        let text_norm = find_input(&inputs, "text_norm", "int32")?;
        let logits = outputs
            .iter()
            .find(|spec| dtype_is_f32(&spec.dtype) && spec.shape.len() == 3)
            .context("SenseVoice 模型缺少 float32[1, T', V] 输出 logits")?;

        let tokens_path = manifest
            .files
            .iter()
            .map(|file| file.path.as_path())
            .find(|path| path.file_name().and_then(|name| name.to_str()) == Some("tokens.txt"))
            .context("SenseVoice 清单缺少 tokens.txt")?;
        let tokens = SenseVoiceTokenizer::load(tokens_path)?;
        let token_count = tokens.token_count();

        let metadata = session
            .metadata()
            .context("读取 SenseVoice 模型 metadata 失败")?;
        let lfr_window = read_usize(&metadata, "lfr_window_size").unwrap_or(LFR_WINDOW);
        let lfr_shift = read_usize(&metadata, "lfr_window_shift").unwrap_or(LFR_SHIFT);
        if lfr_window == 0 || lfr_shift == 0 {
            bail!("SenseVoice LFR 参数必须大于 0");
        }
        if let Ok(vocab_size) = read_usize(&metadata, "vocab_size") {
            if vocab_size != token_count {
                bail!("SenseVoice vocab_size {vocab_size} 与 token_count {token_count} 不一致");
            }
        }
        if logits.shape.last().copied().unwrap_or(0) >= 0
            && logits.shape.last().copied().unwrap_or(0) != token_count as i64
        {
            bail!(
                "SenseVoice logits 最后一维 {:?} 与 token_count {token_count} 不一致",
                logits.shape.last()
            );
        }

        let expected = crate::asr::features::NUM_BINS * lfr_window;
        if x.shape.len() == 3 {
            let last = x.shape[2];
            if last >= 0 && last != expected as i64 {
                bail!("SenseVoice x 最后一维 {last} 与 LFR 维度 {expected} 不一致");
            }
            if x.shape[0] >= 0 && x.shape[0] != 1 {
                bail!("SenseVoice x batch 必须为 1，实际 {}", x.shape[0]);
            }
        }

        let with_itn_id = read_i32(&metadata, "with_itn").unwrap_or(14);
        let without_itn_id = read_i32(&metadata, "without_itn").unwrap_or(15);
        let language_id = match metadata.custom("lang_auto") {
            Some(value) => value
                .trim()
                .parse::<i32>()
                .context("解析 SenseVoice lang_auto 失败")?,
            None => 0,
        };

        let mean = match metadata.custom("neg_mean") {
            Some(value) if !value.trim().is_empty() => {
                parse_cmvn(&value).context("解析 neg_mean 失败")?
            }
            _ => vec![0.0; expected],
        };
        let inv_stddev = match metadata.custom("inv_stddev") {
            Some(value) if !value.trim().is_empty() => {
                parse_cmvn(&value).context("解析 inv_stddev 失败")?
            }
            _ => vec![1.0; expected],
        };
        if mean.len() != expected || inv_stddev.len() != expected {
            bail!(
                "SenseVoice CMVN 维度非法：mean {} / inv_stddev {}，期望 {}",
                mean.len(),
                inv_stddev.len(),
                expected
            );
        }
        drop(metadata);

        Ok(Self {
            session,
            tokens,
            mean,
            inv_stddev,
            lfr_window,
            lfr_shift,
            language_id,
            with_itn_id,
            without_itn_id,
            use_itn: true,
            x_name: x.name.clone(),
            x_length_name: x_length.name.clone(),
            language_name: language.name.clone(),
            text_norm_name: text_norm.name.clone(),
            logits_name: logits.name.clone(),
            token_count,
            fbank,
        })
    }

    /// Computes features from a complete audio segment and transcribes it.
    pub fn transcribe(&mut self, samples: &[f32]) -> anyhow::Result<AsrOutput> {
        if samples.is_empty() {
            return Ok(AsrOutput::default());
        }
        let mut fbank = Fbank::new(self.fbank);
        let features = fbank
            .compute(samples)
            .context("计算 SenseVoice fbank 失败")?;
        self.transcribe_features(&features)
    }

    /// Transcribes pre-computed fbank frames (Preview already has them).
    pub fn transcribe_features(&mut self, features: &[Vec<f32>]) -> anyhow::Result<AsrOutput> {
        if features.is_empty() {
            return Ok(AsrOutput::default());
        }
        validate_features(features)?;
        let mut lfr = features_for_cmvn(features, self.mean.len(), self.lfr_window, self.lfr_shift);
        if lfr.is_empty() {
            return Ok(AsrOutput::default());
        }
        apply_cmvn(&mut lfr, &self.mean, &self.inv_stddev)?;

        let frames = lfr.len();
        let bins = lfr.first().map(Vec::len).unwrap_or(0);
        let mut flat = Vec::with_capacity(frames * bins);
        for (index, frame) in lfr.iter().enumerate() {
            if frame.len() != bins {
                bail!("SenseVoice LFR 帧 {index} 维度不一致");
            }
            flat.extend_from_slice(frame);
        }

        let x = Tensor::from_array(([1usize, frames, bins], flat.into_boxed_slice()))
            .context("构造 SenseVoice x 张量失败")?;
        let x_length = Tensor::from_array(([1usize], vec![frames as i32].into_boxed_slice()))
            .context("构造 SenseVoice x_length 张量失败")?;
        let language = Tensor::from_array(([1usize], vec![self.language_id].into_boxed_slice()))
            .context("构造 SenseVoice language 张量失败")?;
        let text_norm = Tensor::from_array((
            [1usize],
            vec![if self.use_itn {
                self.with_itn_id
            } else {
                self.without_itn_id
            }]
            .into_boxed_slice(),
        ))
        .context("构造 SenseVoice text_norm 张量失败")?;

        let inputs: Vec<(String, SessionInputValue<'_>)> = vec![
            (self.x_name.clone(), x.into_dyn().into()),
            (self.x_length_name.clone(), x_length.into_dyn().into()),
            (self.language_name.clone(), language.into_dyn().into()),
            (self.text_norm_name.clone(), text_norm.into_dyn().into()),
        ];
        let outputs = self
            .session
            .run(inputs)
            .context("SenseVoice ASR 推理失败")?;
        let (shape, logits) = outputs
            .get(&self.logits_name)
            .with_context(|| format!("SenseVoice 输出缺失：{}", self.logits_name))?
            .try_extract_tensor::<f32>()
            .context("SenseVoice logits 输出类型错误")?;
        let dims: &[i64] = shape;
        if dims.len() != 3 || dims[2] <= 0 {
            bail!("SenseVoice logits 形状非法：{dims:?}");
        }
        let time_steps = dims[1].max(0) as usize;
        let vocab = dims[2] as usize;
        if vocab != self.token_count {
            bail!(
                "SenseVoice 输出词表 {vocab} 与 token_count {} 不一致",
                self.token_count
            );
        }
        if logits.len() < time_steps * vocab {
            bail!(
                "SenseVoice logits 数据长度 {} 小于形状 {}x{}",
                logits.len(),
                time_steps,
                vocab
            );
        }
        let valid = time_steps.min(frames + SENSE_VOICE_PREFIX_TOKENS);
        let mut ids = greedy_decode(logits, valid, vocab);
        if ids.len() < SENSE_VOICE_PREFIX_TOKENS {
            bail!(
                "SenseVoice 输出缺少 {} 个前缀 token（实际 {}）",
                SENSE_VOICE_PREFIX_TOKENS,
                ids.len()
            );
        }
        ids.drain(..SENSE_VOICE_PREFIX_TOKENS);
        let text = self.tokens.decode(&ids);
        Ok(AsrOutput { ids, text })
    }

    pub fn reset(&mut self) {}
}

fn find_input<'a>(
    inputs: &'a [crate::infer::session::TensorSpec],
    name: &str,
    dtype: &str,
) -> anyhow::Result<&'a crate::infer::session::TensorSpec> {
    inputs
        .iter()
        .find(|spec| {
            spec.name == name
                && spec.shape.len() == 1
                && if dtype == "int32" {
                    dtype_is_i32(&spec.dtype)
                } else {
                    spec.dtype.eq_ignore_ascii_case(dtype)
                }
        })
        .with_context(|| format!("SenseVoice 模型缺少 {name} 输入"))
}

fn features_for_cmvn(
    features: &[Vec<f32>],
    expected_dim: usize,
    window: usize,
    shift: usize,
) -> Vec<Vec<f32>> {
    if features.first().map(Vec::len) == Some(expected_dim) {
        features.to_vec()
    } else {
        apply_lfr(features, window, shift)
    }
}

fn validate_features(features: &[Vec<f32>]) -> anyhow::Result<()> {
    let bins = features.first().map(Vec::len).unwrap_or(0);
    if bins == 0 {
        bail!("SenseVoice 特征帧为空");
    }
    for (index, frame) in features.iter().enumerate() {
        if frame.len() != bins {
            bail!(
                "SenseVoice 特征帧 {index} 维度 {}，期望 {bins}",
                frame.len()
            );
        }
    }
    Ok(())
}

fn read_usize(metadata: &ort::session::ModelMetadata<'_>, key: &str) -> anyhow::Result<usize> {
    metadata
        .custom(key)
        .with_context(|| format!("SenseVoice 模型缺少 {key} metadata"))?
        .trim()
        .parse::<usize>()
        .with_context(|| format!("解析 SenseVoice {key} 失败"))
}

fn read_i32(metadata: &ort::session::ModelMetadata<'_>, key: &str) -> anyhow::Result<i32> {
    metadata
        .custom(key)
        .with_context(|| format!("SenseVoice 模型缺少 {key} metadata"))?
        .trim()
        .parse::<i32>()
        .with_context(|| format!("解析 SenseVoice {key} 失败"))
}

/// Greedy CTC decode: per-frame argmax, collapse repeats, drop blank 0.
pub fn greedy_decode(logits: &[f32], frames: usize, vocab: usize) -> Vec<usize> {
    let mut ids = Vec::new();
    let mut previous = usize::MAX;
    for frame in 0..frames {
        let start = frame * vocab;
        let end = start + vocab;
        if end > logits.len() {
            break;
        }
        let mut best = 0usize;
        for (index, value) in logits[start..end].iter().enumerate() {
            if *value > logits[start + best] {
                best = index;
            }
        }
        if best != 0 && best != previous {
            ids.push(best);
        }
        previous = best;
    }
    ids
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lfr_is_applied_exactly_once() {
        let fbank: Vec<Vec<f32>> = (0..13).map(|value| vec![value as f32; 80]).collect();
        let lfr = features_for_cmvn(&fbank, 560, 7, 6);
        assert_eq!(lfr.len(), 2);
        assert_eq!(lfr[0].len(), 560);
        let again = features_for_cmvn(&lfr, 560, 7, 6);
        assert_eq!(again, lfr);
    }

    #[test]
    fn greedy_collapses_repeats_and_blank() {
        let logits = vec![
            0.0, 0.9, 0.1, // 1
            0.0, 0.9, 0.1, // 1 (repeat)
            0.9, 0.1, 0.0, // 0 (blank)
            0.0, 0.1, 0.9, // 2
        ];
        assert_eq!(greedy_decode(&logits, 4, 3), vec![1, 2]);
    }
}
