use anyhow::{bail, Context};
use ort::session::{Session, SessionInputValue};
use ort::value::Tensor;

use crate::asr::firered_ctc::{parse_cmvn, AsrOutput};
use crate::asr::tokenizer::CtcTokenizer;
use crate::infer::session::{dtype_is_f32, dtype_is_i64, SessionFactory};
use crate::model::{AsrArtifact, TensorSpec};

/// FireRed AED is an encoder + autoregressive decoder pair. The decoder keeps
/// a self-attention KV cache; the encoder output is the cross-attention cache.
/// This is a CPU greedy implementation matching sherpa-onnx 1.12.27.
pub struct FireRedAed {
    encoder: Session,
    decoder: Session,
    tokens: CtcTokenizer,
    cmvn_mean: Vec<f32>,
    cmvn_inv_stddev: Vec<f32>,
    sos_id: i32,
    eos_id: i32,
    max_len: usize,
    num_decoder_layers: usize,
    num_head: usize,
    head_dim: usize,
    max_token_per_second: usize,
    feature_bins: usize,
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

impl FireRedAed {
    pub fn new_with_factory(
        artifact: &AsrArtifact,
        factory: &SessionFactory,
    ) -> anyhow::Result<Self> {
        let encoder_artifact = artifact
            .encoder
            .as_ref()
            .context("FireRed AED 清单缺少 asr.encoder")?;
        let decoder_artifact = artifact
            .decoder
            .as_ref()
            .context("FireRed AED 清单缺少 asr.decoder")?;
        let encoder = factory.build(encoder_artifact)?;
        let decoder = factory.build(decoder_artifact)?;

        let encoder_x = find_tensor(&encoder_artifact.inputs, &["x"], true, 3)
            .context("FireRed AED encoder 缺少 f32 三维输入 x")?;
        let encoder_len = find_tensor(&encoder_artifact.inputs, &["x_len", "length"], false, 1)
            .context("FireRed AED encoder 缺少 i64 一维输入 x_len")?;
        let encoder_k = find_tensor(&encoder_artifact.outputs, &["cross_k"], true, 4)
            .context("FireRed AED encoder 缺少 f32 四维输出 cross_k")?;
        let encoder_v = find_tensor(&encoder_artifact.outputs, &["cross_v"], true, 4)
            .context("FireRed AED encoder 缺少 f32 四维输出 cross_v")?;

        let tokens_spec = find_tensor(&decoder_artifact.inputs, &["tokens"], false, 2)
            .context("FireRed AED decoder 缺少 i64 二维输入 tokens")?;
        let self_k = find_tensor(&decoder_artifact.inputs, &["self_k"], true, 5)
            .context("FireRed AED decoder 缺少 f32 五维输入 self_k_cache")?;
        let self_v = find_tensor(&decoder_artifact.inputs, &["self_v"], true, 5)
            .context("FireRed AED decoder 缺少 f32 五维输入 self_v_cache")?;
        let cross_k = find_tensor(&decoder_artifact.inputs, &["cross_k"], true, 4)
            .context("FireRed AED decoder 缺少 f32 四维输入 cross_k")?;
        let cross_v = find_tensor(&decoder_artifact.inputs, &["cross_v"], true, 4)
            .context("FireRed AED decoder 缺少 f32 四维输入 cross_v")?;
        let offset = find_tensor(&decoder_artifact.inputs, &["offset"], false, 1)
            .context("FireRed AED decoder 缺少 i64 一维输入 offset")?;
        let logits = find_tensor(&decoder_artifact.outputs, &["logits"], true, 3)
            .context("FireRed AED decoder 缺少 f32 三维输出 logits")?;
        let out_self_k = find_tensor(&decoder_artifact.outputs, &["self_k"], true, 5)
            .context("FireRed AED decoder 缺少 f32 五维输出 self_k_cache")?;
        let out_self_v = find_tensor(&decoder_artifact.outputs, &["self_v"], true, 5)
            .context("FireRed AED decoder 缺少 f32 五维输出 self_v_cache")?;

        let tokens = CtcTokenizer::load(&artifact.tokens)?;
        if tokens.token_count() != artifact.token_count {
            bail!(
                "FireRed AED 词表条目 {} 与清单 token_count {} 不一致",
                tokens.token_count(),
                artifact.token_count
            );
        }
        if logits.shape.last().copied().unwrap_or(0) > 0
            && logits.shape.last().copied().unwrap_or(0) != artifact.token_count as i64
        {
            bail!(
                "FireRed AED logits 最后一维 {:?} 与 token_count {} 不一致",
                logits.shape.last(),
                artifact.token_count
            );
        }

        let metadata = encoder
            .metadata()
            .context("读取 FireRed AED encoder metadata 失败")?;
        let num_decoder_layers = read_usize(&metadata, "num_decoder_layers")?;
        let num_head = read_usize(&metadata, "num_head")?;
        let head_dim = read_usize(&metadata, "head_dim")?;
        let sos_id = read_i32(&metadata, "sos")?;
        let eos_id = read_i32(&metadata, "eos")?;
        let max_len = read_usize(&metadata, "max_len")?;
        if num_decoder_layers == 0 || num_head == 0 || head_dim == 0 || max_len == 0 {
            bail!("FireRed AED metadata 中的缓存维度必须大于 0");
        }
        if sos_id < 0
            || eos_id < 0
            || sos_id as usize >= artifact.token_count
            || eos_id as usize >= artifact.token_count
        {
            bail!("FireRed AED sos/eos token 非法：sos={sos_id} eos={eos_id}");
        }
        let cmvn_mean = metadata
            .custom("cmvn_mean")
            .context("FireRed AED encoder 缺少 cmvn_mean metadata")
            .and_then(|value| parse_cmvn(&value).context("解析 cmvn_mean 失败"))?;
        let cmvn_inv_stddev = metadata
            .custom("cmvn_inv_stddev")
            .context("FireRed AED encoder 缺少 cmvn_inv_stddev metadata")
            .and_then(|value| parse_cmvn(&value).context("解析 cmvn_inv_stddev 失败"))?;
        if cmvn_mean.is_empty() || cmvn_mean.len() != cmvn_inv_stddev.len() {
            bail!(
                "FireRed AED CMVN 维度非法：mean {} / inv_stddev {}",
                cmvn_mean.len(),
                cmvn_inv_stddev.len()
            );
        }
        let feature_bins = encoder_x
            .shape
            .last()
            .copied()
            .filter(|value| *value > 0)
            .map(|value| value as usize)
            .unwrap_or(cmvn_mean.len());
        if feature_bins != cmvn_mean.len() {
            bail!(
                "FireRed AED fbank 维度 {} 与 CMVN 维度 {} 不一致",
                feature_bins,
                cmvn_mean.len()
            );
        }
        drop(metadata);

        Ok(Self {
            encoder,
            decoder,
            tokens,
            cmvn_mean,
            cmvn_inv_stddev,
            sos_id,
            eos_id,
            max_len,
            num_decoder_layers,
            num_head,
            head_dim,
            max_token_per_second: 8,
            feature_bins,
            encoder_x_name: encoder_x.name.clone(),
            encoder_len_name: encoder_len.name.clone(),
            encoder_k_name: encoder_k.name.clone(),
            encoder_v_name: encoder_v.name.clone(),
            tokens_name: tokens_spec.name.clone(),
            self_k_name: self_k.name.clone(),
            self_v_name: self_v.name.clone(),
            cross_k_name: cross_k.name.clone(),
            cross_v_name: cross_v.name.clone(),
            offset_name: offset.name.clone(),
            logits_name: logits.name.clone(),
            out_self_k_name: out_self_k.name.clone(),
            out_self_v_name: out_self_v.name.clone(),
        })
    }

    pub fn transcribe(&mut self, features: &[Vec<f32>]) -> anyhow::Result<AsrOutput> {
        if features.is_empty() {
            return Ok(AsrOutput::default());
        }
        let bins = self.feature_bins;
        let frames = features.len();
        let mut flat = Vec::with_capacity(frames * bins);
        for (index, frame) in features.iter().enumerate() {
            if frame.len() != bins {
                bail!(
                    "FireRed AED fbank 帧 {} 有 {} 个 bin，CMVN 期望 {}",
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
            .context("构造 FireRed AED encoder x 张量失败")?;
        let x_len = Tensor::from_array(([1usize], vec![frames as i64].into_boxed_slice()))
            .context("构造 FireRed AED x_len 张量失败")?;
        let encoder_inputs: Vec<(String, SessionInputValue<'_>)> = vec![
            (self.encoder_x_name.clone(), x.into_dyn().into()),
            (self.encoder_len_name.clone(), x_len.into_dyn().into()),
        ];
        let mut encoder_outputs = self
            .encoder
            .run(encoder_inputs)
            .context("FireRed AED encoder 推理失败")?;
        let cross_k = encoder_outputs
            .remove(&self.encoder_k_name)
            .with_context(|| format!("FireRed AED encoder 输出缺失：{}", self.encoder_k_name))?;
        let cross_v = encoder_outputs
            .remove(&self.encoder_v_name)
            .with_context(|| format!("FireRed AED encoder 输出缺失：{}", self.encoder_v_name))?;
        drop(encoder_outputs);

        let cache_shape = [
            self.num_decoder_layers,
            1,
            self.max_len,
            self.num_head,
            self.head_dim,
        ];
        let cache_len = cache_shape.iter().product::<usize>();
        let mut self_k =
            Tensor::from_array((cache_shape, vec![0.0f32; cache_len].into_boxed_slice()))
                .context("构造 FireRed AED self_k cache 失败")?
                .into_dyn();
        let mut self_v =
            Tensor::from_array((cache_shape, vec![0.0f32; cache_len].into_boxed_slice()))
                .context("构造 FireRed AED self_v cache 失败")?
                .into_dyn();

        let max_tokens = ((frames as f64 / 100.0 * self.max_token_per_second as f64) as usize)
            .max(1)
            .min((self.max_len / 2).max(1));
        let mut ids = Vec::new();
        let mut token = self.sos_id;
        for step in 0..max_tokens {
            let tokens =
                Tensor::from_array(([1usize, 1usize], vec![token as i64].into_boxed_slice()))
                    .context("构造 FireRed AED tokens 张量失败")?;
            let offset_tensor =
                Tensor::from_array(([1usize], vec![step as i64].into_boxed_slice()))
                    .context("构造 FireRed AED offset 张量失败")?;
            let inputs: Vec<(String, SessionInputValue<'_>)> = vec![
                (self.tokens_name.clone(), tokens.into_dyn().into()),
                (self.self_k_name.clone(), (&self_k).into()),
                (self.self_v_name.clone(), (&self_v).into()),
                (self.cross_k_name.clone(), (&cross_k).into()),
                (self.cross_v_name.clone(), (&cross_v).into()),
                (self.offset_name.clone(), offset_tensor.into_dyn().into()),
            ];
            let mut outputs = self
                .decoder
                .run(inputs)
                .context("FireRed AED decoder 推理失败")?;
            let logits = outputs
                .remove(&self.logits_name)
                .with_context(|| format!("FireRed AED decoder 输出缺失：{}", self.logits_name))?;
            let (shape, logits) = logits
                .try_extract_tensor::<f32>()
                .context("FireRed AED logits 输出类型错误")?;
            let dims: &[i64] = shape;
            if dims.len() != 3 || dims[2] <= 0 {
                bail!("FireRed AED logits 形状非法：{:?}", dims);
            }
            let vocab = dims[2] as usize;
            if vocab != self.tokens.token_count() {
                bail!(
                    "FireRed AED logits 词表 {} 与 tokens {} 不一致",
                    vocab,
                    self.tokens.token_count()
                );
            }
            if logits.len() < vocab {
                bail!(
                    "FireRed AED logits 数据长度 {} 小于词表 {}",
                    logits.len(),
                    vocab
                );
            }
            let best = argmax(&logits[..vocab]);
            if best == self.eos_id as usize {
                break;
            }
            ids.push(best);
            token = best as i32;
            self_k = outputs.remove(&self.out_self_k_name).with_context(|| {
                format!("FireRed AED decoder 输出缺失：{}", self.out_self_k_name)
            })?;
            self_v = outputs.remove(&self.out_self_v_name).with_context(|| {
                format!("FireRed AED decoder 输出缺失：{}", self.out_self_v_name)
            })?;
        }
        let text = self.tokens.decode(&ids);
        Ok(AsrOutput { ids, text })
    }
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

fn find_tensor<'a>(
    specs: &'a [TensorSpec],
    needles: &[&str],
    want_f32: bool,
    rank: usize,
) -> Option<&'a TensorSpec> {
    let matches = |spec: &&TensorSpec| {
        spec.shape.len() == rank
            && if want_f32 {
                dtype_is_f32(&spec.dtype)
            } else {
                dtype_is_i64(&spec.dtype)
            }
    };
    for needle in needles {
        if let Some(spec) = specs
            .iter()
            .find(|spec| matches(spec) && spec.name.to_ascii_lowercase().contains(needle))
        {
            return Some(spec);
        }
    }
    specs.iter().find(matches)
}

fn read_usize(metadata: &ort::session::ModelMetadata<'_>, key: &str) -> anyhow::Result<usize> {
    metadata
        .custom(key)
        .with_context(|| format!("FireRed AED 模型缺少 {key} metadata"))?
        .trim()
        .parse::<usize>()
        .with_context(|| format!("解析 FireRed AED {key} 失败"))
}

fn read_i32(metadata: &ort::session::ModelMetadata<'_>, key: &str) -> anyhow::Result<i32> {
    metadata
        .custom(key)
        .with_context(|| format!("FireRed AED 模型缺少 {key} metadata"))?
        .trim()
        .parse::<i32>()
        .with_context(|| format!("解析 FireRed AED {key} 失败"))
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
    fn finds_tensors_by_name_and_shape() {
        let specs = vec![
            TensorSpec {
                name: "n_layer_cross_k".to_string(),
                dtype: "float32".to_string(),
                shape: vec![16, 1, -1, 1280],
            },
            TensorSpec {
                name: "n_layer_cross_v".to_string(),
                dtype: "float32".to_string(),
                shape: vec![16, 1, -1, 1280],
            },
        ];
        assert_eq!(
            find_tensor(&specs, &["cross_k"], true, 4).map(|spec| spec.name.as_str()),
            Some("n_layer_cross_k")
        );
        assert_eq!(
            find_tensor(&specs, &["cross_v"], true, 4).map(|spec| spec.name.as_str()),
            Some("n_layer_cross_v")
        );
    }
}
