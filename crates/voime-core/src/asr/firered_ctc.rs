use anyhow::{bail, Context};
use ort::session::{Session, SessionInputValue};
use ort::value::Tensor;

use crate::asr::tokenizer::CtcTokenizer;
use crate::infer::session::{dtype_is_f32, dtype_is_i64, SessionFactory};
use crate::model::CtcArtifact;

#[derive(Clone, Debug, Default)]
pub struct AsrOutput {
    pub ids: Vec<usize>,
    pub text: String,
}

pub struct FireRedCtc {
    session: Session,
    tokens: CtcTokenizer,
    cmvn_mean: Vec<f32>,
    cmvn_inv_stddev: Vec<f32>,
    x_name: String,
    x_lens_name: String,
    log_probs_name: String,
    log_probs_len_name: String,
}

impl FireRedCtc {
    /// Loads the CTC model using the manifest-declared tensor names.
    pub fn new_with_factory(
        artifact: &CtcArtifact,
        factory: &SessionFactory,
    ) -> anyhow::Result<Self> {
        let session = factory.build(&artifact.model)?;
        let inputs = &artifact.model.inputs;
        let x = inputs
            .iter()
            .find(|spec| {
                dtype_is_f32(&spec.dtype)
                    && spec.shape.len() == 3
                    && spec.shape.last().copied().unwrap_or(0) > 0
            })
            .context("ASR 清单缺少 [1, T, 80] 的 fbank 输入 x")?;
        let x_lens = inputs
            .iter()
            .find(|spec| dtype_is_i64(&spec.dtype) && spec.shape.len() == 1)
            .context("ASR 清单缺少 int64[1] 输入 x_lens")?;
        let outputs = &artifact.model.outputs;
        let log_probs = outputs
            .iter()
            .find(|spec| dtype_is_f32(&spec.dtype) && spec.shape.len() == 3)
            .context("ASR 清单缺少 float32[1, T', V] 输出 log_probs")?;
        let log_probs_len = outputs
            .iter()
            .find(|spec| dtype_is_i64(&spec.dtype) && spec.shape.len() == 1)
            .context("ASR 清单缺少 int64[1] 输出 log_probs_len")?;

        let tokens = CtcTokenizer::load(&artifact.tokens)?;
        if tokens.token_count() != artifact.token_count {
            bail!(
                "CTC 词表条目 {} 与清单 token_count {} 不一致",
                tokens.token_count(),
                artifact.token_count
            );
        }
        let (cmvn_mean, cmvn_inv_stddev) = read_cmvn(&session)?;
        Ok(Self {
            session,
            tokens,
            cmvn_mean,
            cmvn_inv_stddev,
            x_name: x.name.clone(),
            x_lens_name: x_lens.name.clone(),
            log_probs_name: log_probs.name.clone(),
            log_probs_len_name: log_probs_len.name.clone(),
        })
    }

    pub fn transcribe(&mut self, features: &[Vec<f32>]) -> anyhow::Result<AsrOutput> {
        if features.is_empty() {
            return Ok(AsrOutput::default());
        }
        let bins = self.cmvn_mean.len();
        if self.cmvn_inv_stddev.len() != bins {
            bail!("CMVN 均值与方差长度不一致");
        }
        let frames = features.len();
        let mut flat = Vec::with_capacity(frames * bins);
        for (index, frame) in features.iter().enumerate() {
            if frame.len() != bins {
                bail!(
                    "fbank 帧 {} 有 {} 个 bin，CMVN 期望 {}",
                    index,
                    frame.len(),
                    bins
                );
            }
            for (bin, value) in frame.iter().enumerate() {
                flat.push((*value - self.cmvn_mean[bin]) * self.cmvn_inv_stddev[bin]);
            }
        }

        let x = Tensor::from_array(([1usize, frames, bins], flat.into_boxed_slice()))
            .context("构造 CTC x 张量失败")?;
        let x_lens = Tensor::from_array(([1usize], vec![frames as i64].into_boxed_slice()))
            .context("构造 CTC x_lens 张量失败")?;
        let inputs: Vec<(String, SessionInputValue<'_>)> = vec![
            (self.x_name.clone(), x.into_dyn().into()),
            (self.x_lens_name.clone(), x_lens.into_dyn().into()),
        ];
        let outputs = self.session.run(inputs).context("CTC 推理失败")?;

        let (shape, log_probs) = outputs
            .get(&self.log_probs_name)
            .with_context(|| format!("CTC 输出缺失：{}", self.log_probs_name))?
            .try_extract_tensor::<f32>()
            .context("CTC log_probs 输出类型错误")?;
        let dims: &[i64] = shape;
        if dims.len() != 3 || dims[2] <= 0 {
            bail!("CTC log_probs 形状非法：{:?}", dims);
        }
        let time_steps = dims[1].max(0) as usize;
        let vocab = dims[2] as usize;
        let len_output = outputs
            .get(&self.log_probs_len_name)
            .with_context(|| format!("CTC 输出缺失：{}", self.log_probs_len_name))?
            .try_extract_tensor::<i64>()
            .context("CTC log_probs_len 输出类型错误")?;
        let valid = len_output.1.first().copied().unwrap_or(0).max(0) as usize;
        let valid = valid.min(time_steps);
        if log_probs.len() < time_steps * vocab {
            bail!(
                "CTC log_probs 数据长度 {} 小于形状 {}x{}",
                log_probs.len(),
                time_steps,
                vocab
            );
        }

        let ids = greedy_decode(log_probs, valid, vocab);
        let text = self.tokens.decode(&ids);
        Ok(AsrOutput { ids, text })
    }
}

/// Greedy CTC: per-frame argmax, merge consecutive repeats, drop blank 0.
pub fn greedy_decode(log_probs: &[f32], frames: usize, vocab: usize) -> Vec<usize> {
    if vocab == 0 {
        return Vec::new();
    }
    let mut ids = Vec::new();
    let mut previous = usize::MAX;
    for frame in 0..frames {
        let row = &log_probs[frame * vocab..(frame + 1) * vocab];
        let mut best = 0usize;
        for (index, value) in row.iter().enumerate() {
            if *value > row[best] {
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

fn read_cmvn(session: &Session) -> anyhow::Result<(Vec<f32>, Vec<f32>)> {
    let metadata = session.metadata().context("读取 CTC 模型 metadata 失败")?;
    let mean = metadata
        .custom("cmvn_mean")
        .context("CTC 模型缺少 cmvn_mean metadata")?;
    let inv_stddev = metadata
        .custom("cmvn_inv_stddev")
        .context("CTC 模型缺少 cmvn_inv_stddev metadata")?;
    let mean = parse_cmvn(&mean).context("解析 cmvn_mean 失败")?;
    let inv_stddev = parse_cmvn(&inv_stddev).context("解析 cmvn_inv_stddev 失败")?;
    if mean.is_empty() || mean.len() != inv_stddev.len() {
        bail!(
            "CMVN 维度非法：mean {} / inv_stddev {}",
            mean.len(),
            inv_stddev.len()
        );
    }
    Ok((mean, inv_stddev))
}

/// Accepts either a JSON array or whitespace/comma separated floats, which
/// covers the metadata encodings used by sherpa-onnx exports.
pub fn parse_cmvn(text: &str) -> anyhow::Result<Vec<f32>> {
    let text = text.trim();
    let values: Vec<f32> = if text.starts_with('[') {
        serde_json::from_str(text).context("CMVN JSON 数组格式错误")?
    } else {
        text.split(|character: char| {
            character.is_whitespace() || character == ',' || character == ';'
        })
        .filter(|part| !part.is_empty())
        .map(|part| {
            part.parse::<f32>()
                .with_context(|| format!("CMVN 数值非法：{part}"))
        })
        .collect::<anyhow::Result<Vec<_>>>()?
    };
    if values.is_empty() {
        bail!("CMVN 数据为空");
    }
    Ok(values)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn greedy_filters_blank_and_repeats() {
        // three frames, four classes
        let logits = vec![
            0.0, 0.9, 0.1, 0.0, // class 1
            0.0, 0.8, 0.2, 0.0, // class 1 -> repeated, dropped
            0.0, 0.0, 1.0, 0.0, // class 2
            1.0, 0.0, 0.0, 0.0, // blank
            0.0, 0.0, 0.0, 1.0, // class 3
        ];
        assert_eq!(greedy_decode(&logits, 5, 4), vec![1, 2, 3]);
    }

    #[test]
    fn blank_between_repeats_keeps_both() {
        let logits = vec![
            0.0, 1.0, 0.9, 0.0, // class 1
            1.0, 0.0, 0.0, 0.0, // blank
            0.0, 1.0, 0.9, 0.0, // class 1 again
        ];
        assert_eq!(greedy_decode(&logits, 3, 4), vec![1, 1]);
    }

    #[test]
    fn consecutive_blanks_are_dropped() {
        let logits = vec![
            0.0, 1.0, 0.0, 0.0, // class 1
            1.0, 0.0, 0.0, 0.0, // blank
            1.0, 0.0, 0.0, 0.0, // blank
            0.0, 0.0, 1.0, 0.0, // class 2
        ];
        assert_eq!(greedy_decode(&logits, 4, 4), vec![1, 2]);
    }

    #[test]
    fn parses_json_and_space_separated_cmvn() {
        assert_eq!(parse_cmvn("[1.0, 2.5, -3]").unwrap(), vec![1.0, 2.5, -3.0]);
        assert_eq!(parse_cmvn("1 2.5\t-3").unwrap(), vec![1.0, 2.5, -3.0]);
        assert!(parse_cmvn("").is_err());
    }
}
