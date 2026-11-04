use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::{bail, Context};
use ort::session::{Session, SessionInputValue};
use ort::value::Tensor;

use crate::asr::cmvn::Cmvn;
use crate::asr::fbank::{Fbank, FbankConfig};
use crate::asr::tokenizer::AedTokenizer;
use crate::model::{ModelManifest, TensorSpec};
use crate::runtime::Runtime;

pub const CACHE_LAYERS: usize = 16;
const ENCODER_DIM: usize = 1280;
const MAX_ENCODER_FRAMES: usize = 5000;

#[derive(Clone, Copy, Debug)]
pub struct AedConfig {
    pub max_tokens: usize,
    pub sos_id: i64,
    pub eos_id: i64,
}

pub struct EncoderOutput {
    pub output: Vec<f32>,
    pub mask: Vec<f32>,
    pub frames: usize,
}

pub struct Aed {
    encoder: Session,
    decoder: Session,
    tokenizer: AedTokenizer,
    cmvn: Cmvn,
    fbank: Fbank,
    config: AedConfig,
}

impl Aed {
    pub fn new(manifest: &ModelManifest, runtime: &Runtime) -> anyhow::Result<Self> {
        let encoder_path = manifest.file("encoder.onnx")?;
        let decoder_path = manifest.file("decoder.onnx")?;
        let dict = manifest.file("dict.txt")?;
        let bpe = manifest.file("train_bpe1000.model")?;
        let cmvn_path = manifest.file("cmvn.ark")?;
        let config = load_config(manifest.file("config.json")?)?;

        let encoder_inputs = vec![
            spec("input", "float32", &[1, -1, 80]),
            spec("input_lengths", "int64", &[1]),
        ];
        let encoder_outputs = vec![
            spec("output", "float32", &[1, -1, ENCODER_DIM as i64]),
            spec("output_lengths", "int64", &[1]),
            spec("mask", "float32", &[1, 1, -1]),
        ];
        let mut decoder_inputs = vec![
            spec("ys", "int64", &[1, -1]),
            spec("encoder_output", "float32", &[1, -1, ENCODER_DIM as i64]),
            spec("src_mask", "float32", &[1, 1, -1]),
        ];
        for index in 0..CACHE_LAYERS {
            decoder_inputs.push(spec(
                &format!("cache_{index}"),
                "float32",
                &[1, -1, ENCODER_DIM as i64],
            ));
        }
        let mut decoder_outputs = vec![spec("logits", "float32", &[1, -1])];
        for index in 0..CACHE_LAYERS {
            decoder_outputs.push(spec(
                &format!("new_cache_{index}"),
                "float32",
                &[1, -1, ENCODER_DIM as i64],
            ));
        }
        let encoder = runtime.create_with_spec(encoder_path, &encoder_inputs, &encoder_outputs)?;
        let decoder = runtime.create_with_spec(decoder_path, &decoder_inputs, &decoder_outputs)?;
        let tokenizer = AedTokenizer::load(dict, bpe)?;
        if let Some(last) = decoder
            .outputs()
            .iter()
            .find(|outlet| outlet.name() == "logits")
        {
            let vocab = match last.dtype() {
                ort::value::ValueType::Tensor { shape, .. } => shape.last().copied().unwrap_or(0),
                _ => 0,
            };
            if vocab > 0 && vocab as usize != tokenizer.token_count() {
                bail!(
                    "AED logits 词表 {} 与 dict.txt {} 不一致",
                    vocab,
                    tokenizer.token_count()
                );
            }
        }
        let cmvn = Cmvn::load(cmvn_path)?;
        Ok(Self {
            encoder,
            decoder,
            tokenizer,
            cmvn,
            fbank: Fbank::new(FbankConfig::default()),
            config,
        })
    }

    pub fn transcribe(&mut self, samples: &[f32], cancel: &AtomicBool) -> anyhow::Result<String> {
        if samples.is_empty() {
            return Ok(String::new());
        }
        let features = self.fbank.compute(samples)?;
        let mut features = features;
        self.cmvn.apply(&mut features)?;
        if features.len() > MAX_ENCODER_FRAMES {
            bail!("AED 输入帧数超过上限：{}", features.len());
        }
        let encoder = self.encode(&features)?;
        if cancel.load(Ordering::Relaxed) {
            bail!("cancelled");
        }
        let ids = self.decode_greedy(&encoder, cancel)?;
        self.tokenizer.decode(&ids)
    }

    fn encode(&mut self, features: &[Vec<f32>]) -> anyhow::Result<EncoderOutput> {
        let frames = features.len();
        let mut flat = Vec::with_capacity(frames * ENCODER_DIM.min(80));
        for frame in features {
            flat.extend_from_slice(frame);
        }
        let bins = features.first().map(Vec::len).unwrap_or(80);
        let x = Tensor::from_array(([1usize, frames, bins], flat.into_boxed_slice()))
            .context("构造 AED encoder 输入失败")?;
        let lengths = Tensor::from_array(([1usize], vec![frames as i64].into_boxed_slice()))
            .context("构造 AED input_lengths 失败")?;
        let inputs: Vec<(String, SessionInputValue<'_>)> = vec![
            ("input".to_string(), x.into_dyn().into()),
            ("input_lengths".to_string(), lengths.into_dyn().into()),
        ];
        let outputs = self.encoder.run(inputs).context("AED encoder 推理失败")?;
        let (shape, output) = outputs
            .get("output")
            .context("AED encoder 缺少 output")?
            .try_extract_tensor::<f32>()
            .context("AED encoder output 类型错误")?;
        let (_, mask) = outputs
            .get("mask")
            .context("AED encoder 缺少 mask")?
            .try_extract_tensor::<f32>()
            .context("AED encoder mask 类型错误")?;
        let frames = if shape.len() >= 2 {
            shape[1].max(0) as usize
        } else {
            0
        };
        Ok(EncoderOutput {
            output: output.to_vec(),
            mask: mask.to_vec(),
            frames,
        })
    }

    fn decode_greedy(
        &mut self,
        encoder: &EncoderOutput,
        cancel: &AtomicBool,
    ) -> anyhow::Result<Vec<i64>> {
        let mut tokens = vec![self.config.sos_id];
        let mut caches: Vec<Vec<f32>> = vec![Vec::new(); CACHE_LAYERS];
        let mut cache_len = 0usize;

        for _ in 0..self.config.max_tokens {
            if cancel.load(Ordering::Relaxed) {
                bail!("cancelled");
            }
            let ys =
                Tensor::from_array(([1usize, tokens.len()], tokens.clone().into_boxed_slice()))
                    .context("构造 AED ys 张量失败")?;
            let encoder_output = Tensor::from_array((
                [1usize, encoder.frames, ENCODER_DIM],
                encoder.output.clone().into_boxed_slice(),
            ))
            .context("构造 AED encoder_output 张量失败")?;
            let src_mask = Tensor::from_array((
                [1usize, 1usize, encoder.frames],
                encoder.mask.clone().into_boxed_slice(),
            ))
            .context("构造 AED src_mask 张量失败")?;
            let mut inputs: Vec<(String, SessionInputValue<'_>)> = vec![
                ("ys".to_string(), ys.into_dyn().into()),
                (
                    "encoder_output".to_string(),
                    encoder_output.into_dyn().into(),
                ),
                ("src_mask".to_string(), src_mask.into_dyn().into()),
            ];
            for (index, cache) in caches.iter().enumerate() {
                let length = if cache.is_empty() { 0 } else { cache_len };
                let tensor = Tensor::from_array((
                    [1usize, length, ENCODER_DIM],
                    cache.clone().into_boxed_slice(),
                ))
                .with_context(|| format!("构造 AED cache_{index} 失败"))?;
                inputs.push((format!("cache_{index}"), tensor.into_dyn().into()));
            }
            let outputs = self.decoder.run(inputs).context("AED decoder 推理失败")?;
            let (shape, logits) = outputs
                .get("logits")
                .context("AED decoder 缺少 logits")?
                .try_extract_tensor::<f32>()
                .context("AED decoder logits 类型错误")?;
            let vocab = shape.last().copied().unwrap_or(0).max(0) as usize;
            if vocab == 0 || logits.len() < vocab {
                bail!("AED logits 形状非法：{shape:?}");
            }
            let row = &logits[logits.len() - vocab..];
            let mut best = 0usize;
            for (index, value) in row.iter().enumerate() {
                if *value > row[best] {
                    best = index;
                }
            }
            tokens.push(best as i64);
            if best as i64 == self.config.eos_id {
                break;
            }
            cache_len = tokens.len() - 1;
            for index in 0..CACHE_LAYERS {
                let name = format!("new_cache_{index}");
                let value = outputs
                    .get(&name)
                    .with_context(|| format!("AED decoder 缺少 {name}"))?;
                let (_shape, data) = value
                    .try_extract_tensor::<f32>()
                    .with_context(|| format!("AED {name} 类型错误"))?;
                caches[index] = data.to_vec();
            }
        }
        if tokens.first().copied() == Some(self.config.eos_id) {
            tokens.clear();
        } else {
            tokens.remove(0);
            if tokens.last().copied() == Some(self.config.eos_id) {
                tokens.pop();
            }
        }
        Ok(tokens)
    }
}

fn spec(name: &str, dtype: &str, shape: &[i64]) -> TensorSpec {
    TensorSpec {
        name: name.to_string(),
        dtype: dtype.to_string(),
        shape: shape.to_vec(),
    }
}

fn load_config(path: &Path) -> anyhow::Result<AedConfig> {
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("无法读取 AED config.json {}", path.display()))?;
    let value: serde_json::Value = serde_json::from_str(&text)
        .with_context(|| format!("AED config.json 格式错误 {}", path.display()))?;
    let max_tokens = value
        .get("output_length_max")
        .and_then(|value| value.as_u64())
        .unwrap_or(300) as usize;
    let sos_id = value
        .get("sos_id")
        .or_else(|| value.get("sos"))
        .and_then(|value| value.as_i64())
        .unwrap_or(3);
    let eos_id = value
        .get("eos_id")
        .or_else(|| value.get("eos"))
        .and_then(|value| value.as_i64())
        .unwrap_or(4);
    if sos_id != 3 || eos_id != 4 {
        bail!("AED 特殊 token id 必须为 <sos>=3 / <eos>=4");
    }
    Ok(AedConfig {
        max_tokens: max_tokens.clamp(1, 1024),
        sos_id,
        eos_id,
    })
}
