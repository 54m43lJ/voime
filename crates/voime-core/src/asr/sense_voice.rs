use anyhow::{bail, Context};
use ort::session::{Session, SessionInputValue};
use ort::value::Tensor;

use crate::asr::r#final::AsrOutput;
use crate::asr::tokenizer::AsrTokenizer;
use crate::infer::session::{dtype_is_f32, dtype_is_i32, SessionFactory};
use crate::model::SenseVoiceManifest;

/// SenseVoice prepends four CTC tokens: language, emotion, event and text
/// normalization. They are metadata, not transcript text.
pub const SENSE_VOICE_PREFIX_TOKENS: usize = 4;

pub struct SenseVoice {
    session: Session,
    tokens: AsrTokenizer,
    neg_mean: Vec<f32>,
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
}

impl SenseVoice {
    pub fn new_with_factory(
        manifest: &SenseVoiceManifest,
        factory: &SessionFactory,
    ) -> anyhow::Result<Self> {
        let model = &manifest.model;
        let session = factory.build(model)?;

        let x = model
            .inputs
            .iter()
            .find(|spec| {
                dtype_is_f32(&spec.dtype)
                    && spec.shape.len() == 3
                    && spec.shape.last().copied().unwrap_or(0) > 0
            })
            .context("SenseVoice 清单缺少 [1, T, 560] 的 f32 输入 x")?;
        let x_length = find_input(model, "x_length", "int32")?;
        let language = find_input(model, "language", "int32")?;
        let text_norm = find_input(model, "text_norm", "int32")?;
        let logits = model
            .outputs
            .iter()
            .find(|spec| dtype_is_f32(&spec.dtype) && spec.shape.len() == 3)
            .context("SenseVoice 清单缺少 float32[1, T', V] 输出 logits")?;

        let tokens = AsrTokenizer::load(&manifest.tokens)?;
        if tokens.token_count() != manifest.token_count {
            bail!(
                "SenseVoice 词表条目 {} 与清单 token_count {} 不一致",
                tokens.token_count(),
                manifest.token_count
            );
        }

        let metadata = session
            .metadata()
            .context("读取 SenseVoice 模型 metadata 失败")?;
        let lfr_window = read_usize(&metadata, "lfr_window_size")?;
        let lfr_shift = read_usize(&metadata, "lfr_window_shift")?;
        if lfr_window == 0 || lfr_shift == 0 {
            bail!("SenseVoice LFR 参数必须大于 0");
        }
        let vocab_size = read_usize(&metadata, "vocab_size")?;
        if vocab_size != manifest.token_count {
            bail!(
                "SenseVoice vocab_size {} 与 token_count {} 不一致",
                vocab_size,
                manifest.token_count
            );
        }
        if logits.shape.last().copied().unwrap_or(0) >= 0
            && logits.shape.last().copied().unwrap_or(0) != vocab_size as i64
        {
            bail!(
                "SenseVoice logits 最后一维 {:?} 与 vocab_size {} 不一致",
                logits.shape.last(),
                vocab_size
            );
        }

        let blank_id = match metadata.custom("blank_id") {
            Some(value) if !value.trim().is_empty() => value
                .trim()
                .parse::<usize>()
                .context("解析 SenseVoice blank_id 失败")?,
            Some(_) | None => 0,
        };
        if blank_id != 0 {
            bail!("当前 SenseVoice 后端只支持 blank_id = 0（模型为 {blank_id}）");
        }

        let normalize_samples = read_i32(&metadata, "normalize_samples")?;
        if normalize_samples != 0 {
            bail!(
                "SenseVoice normalize_samples 必须为 0（fbank 已经乘 32768），实际 {}",
                normalize_samples
            );
        }

        let with_itn_id = read_i32(&metadata, "with_itn")?;
        let without_itn_id = read_i32(&metadata, "without_itn")?;
        for key in [
            "lang_auto",
            "lang_zh",
            "lang_en",
            "lang_ja",
            "lang_ko",
            "lang_yue",
        ] {
            let _ = read_i32(&metadata, key)?;
        }
        let language_id = language_id(&metadata, manifest.options.language.as_deref())?;

        let neg_mean = metadata
            .custom("neg_mean")
            .context("SenseVoice 模型缺少 neg_mean metadata")
            .and_then(|value| parse_cmvn(&value).context("解析 neg_mean 失败"))?;
        let inv_stddev = metadata
            .custom("inv_stddev")
            .context("SenseVoice 模型缺少 inv_stddev metadata")
            .and_then(|value| parse_cmvn(&value).context("解析 inv_stddev 失败"))?;
        let expected = crate::asr::features::NUM_BINS * lfr_window;
        if neg_mean.len() != expected || inv_stddev.len() != expected {
            bail!(
                "SenseVoice CMVN 维度非法：neg_mean {} / inv_stddev {}，期望 {}",
                neg_mean.len(),
                inv_stddev.len(),
                expected
            );
        }
        drop(metadata);

        Ok(Self {
            session,
            tokens,
            neg_mean,
            inv_stddev,
            lfr_window,
            lfr_shift,
            language_id,
            with_itn_id,
            without_itn_id,
            use_itn: manifest.options.use_itn,
            x_name: x.name.clone(),
            x_length_name: x_length.name.clone(),
            language_name: language.name.clone(),
            text_norm_name: text_norm.name.clone(),
            logits_name: logits.name.clone(),
            token_count: manifest.token_count,
        })
    }

    pub fn transcribe(&mut self, features: &[Vec<f32>]) -> anyhow::Result<AsrOutput> {
        if features.is_empty() {
            return Ok(AsrOutput::default());
        }
        let mut lfr = apply_lfr(features, self.lfr_window, self.lfr_shift)?;
        if lfr.is_empty() {
            return Ok(AsrOutput::default());
        }
        apply_cmvn(&mut lfr, &self.neg_mean, &self.inv_stddev)?;

        let frames = lfr.len();
        let bins = lfr.first().map(Vec::len).unwrap_or(0);
        let mut flat = Vec::with_capacity(frames * bins);
        for (index, frame) in lfr.iter().enumerate() {
            if frame.len() != bins {
                bail!("SenseVoice LFR 帧 {} 维度不一致", index);
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
            bail!("SenseVoice logits 形状非法：{:?}", dims);
        }
        let time_steps = dims[1].max(0) as usize;
        let vocab = dims[2] as usize;
        if vocab != self.token_count {
            bail!(
                "SenseVoice 输出词表 {} 与清单 token_count {} 不一致",
                vocab,
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
}

fn find_input<'a>(
    model: &'a crate::model::Artifact,
    name: &str,
    dtype: &str,
) -> anyhow::Result<&'a crate::model::TensorSpec> {
    model
        .inputs
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
        .with_context(|| format!("SenseVoice 清单缺少 {name} 输入"))
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

fn language_id(
    metadata: &ort::session::ModelMetadata<'_>,
    language: Option<&str>,
) -> anyhow::Result<i32> {
    let key = match language.unwrap_or("auto") {
        "auto" => "lang_auto",
        "zh" => "lang_zh",
        "en" => "lang_en",
        "ja" => "lang_ja",
        "ko" => "lang_ko",
        "yue" => "lang_yue",
        other => bail!("SenseVoice 不支持语言：{other}"),
    };
    read_i32(metadata, key)
}

/// Applies the SenseVoice 7/6 LFR front end from sherpa-onnx 1.12.27.
///
/// Only complete windows are emitted, so the result has
/// `(frames - window) / shift + 1` frames. Audio shorter than one window yields
/// no frames.
pub fn apply_lfr(
    features: &[Vec<f32>],
    window: usize,
    shift: usize,
) -> anyhow::Result<Vec<Vec<f32>>> {
    if features.is_empty() {
        return Ok(Vec::new());
    }
    if window == 0 || shift == 0 {
        bail!("LFR window/shift 必须大于 0");
    }
    let bins = features[0].len();
    if bins == 0 {
        bail!("LFR 输入帧为空");
    }
    for (index, frame) in features.iter().enumerate() {
        if frame.len() != bins {
            bail!("LFR 输入帧 {} 维度 {}，期望 {}", index, frame.len(), bins);
        }
    }
    if features.len() < window {
        return Ok(Vec::new());
    }
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
    Ok(output)
}

/// Applies `(x + neg_mean) * inv_stddev` after LFR.
pub fn apply_cmvn(
    frames: &mut [Vec<f32>],
    neg_mean: &[f32],
    inv_stddev: &[f32],
) -> anyhow::Result<()> {
    if neg_mean.len() != inv_stddev.len() {
        bail!(
            "CMVN neg_mean {} 与 inv_stddev {} 长度不一致",
            neg_mean.len(),
            inv_stddev.len()
        );
    }
    for (index, frame) in frames.iter_mut().enumerate() {
        if frame.len() != neg_mean.len() {
            bail!(
                "CMVN 帧 {} 维度 {}，期望 {}",
                index,
                frame.len(),
                neg_mean.len()
            );
        }
        for (bin, value) in frame.iter_mut().enumerate() {
            *value = (*value + neg_mean[bin]) * inv_stddev[bin];
        }
    }
    Ok(())
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

/// Parses the CMVN metadata emitted by sherpa-onnx. Accepts a JSON array or a
/// whitespace / comma / semicolon separated float string.
pub fn parse_cmvn(text: &str) -> anyhow::Result<Vec<f32>> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Ok(Vec::new());
    }
    if trimmed.starts_with('[') {
        let values: Vec<f32> = serde_json::from_str(trimmed).context("解析 CMVN JSON 数组失败")?;
        return Ok(values);
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
    fn lfr_uses_7_by_6_and_emits_complete_windows() {
        let features: Vec<Vec<f32>> = (0..13).map(|value| vec![value as f32]).collect();
        let output = apply_lfr(&features, 7, 6).unwrap();
        assert_eq!(output.len(), 2);
        assert_eq!(output[0], vec![0.0, 1.0, 2.0, 3.0, 4.0, 5.0, 6.0]);
        assert_eq!(output[1], vec![6.0, 7.0, 8.0, 9.0, 10.0, 11.0, 12.0]);
    }

    #[test]
    fn lfr_drops_incomplete_tail() {
        let features: Vec<Vec<f32>> = (0..10).map(|value| vec![value as f32]).collect();
        let output = apply_lfr(&features, 7, 6).unwrap();
        assert_eq!(output.len(), 1);
        assert_eq!(output[0], vec![0.0, 1.0, 2.0, 3.0, 4.0, 5.0, 6.0]);
        assert!(apply_lfr(&features[..4], 7, 6).unwrap().is_empty());
    }

    #[test]
    fn cmvn_uses_added_negative_mean() {
        let mut frames = vec![vec![1.0, 2.0], vec![3.0, 4.0]];
        apply_cmvn(&mut frames, &[-1.0, 0.5], &[2.0, 4.0]).unwrap();
        assert_eq!(frames, vec![vec![0.0, 10.0], vec![4.0, 18.0]]);
        assert!(apply_cmvn(&mut frames, &[0.0], &[1.0]).is_err());
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

    #[test]
    fn lfr_rejects_ragged_input() {
        let features = vec![vec![0.0], vec![0.0, 1.0]];
        assert!(apply_lfr(&features, 7, 6).is_err());
    }
}
