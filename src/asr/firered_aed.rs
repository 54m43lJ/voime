use anyhow::{bail, Context};
use ort::session::{Session, SessionInputValue};
use ort::value::{Tensor, TensorElementType, ValueType};

use crate::asr::decoder::{beam_search, Hypothesis};
use crate::asr::features::FbankConfig;
use crate::asr::tokenizer::Tokenizer;
use crate::infer::session::SessionFactory;
use crate::model::AsrArtifact;

pub const BEAM_SIZE: usize = 3;
pub const MAX_DECODE_LEN: usize = 256;
pub const MAX_AUDIO_MS: u64 = 60_000;

#[derive(Clone, Debug)]
pub struct AedConfig {
    pub beam_size: usize,
    pub max_decode_len: usize,
    pub sos_id: u32,
    pub eos_id: u32,
    pub pad_id: u32,
    pub fbank: FbankConfig,
}

impl Default for AedConfig {
    fn default() -> Self {
        Self {
            beam_size: BEAM_SIZE,
            max_decode_len: MAX_DECODE_LEN,
            sos_id: 1,
            eos_id: 2,
            pad_id: 0,
            fbank: FbankConfig::default(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct AsrOutput {
    pub text: String,
    pub confidence: Option<f32>,
}

pub struct FireRedAed {
    encoder: Session,
    decoder: Session,
    tokenizer: Tokenizer,
    config: AedConfig,
}

impl FireRedAed {
    pub fn new(
        artifact: &AsrArtifact,
        factory: &SessionFactory,
        mut config: AedConfig,
    ) -> anyhow::Result<Self> {
        let encoder = factory.build(&artifact.encoder)?;
        let decoder = factory.build(&artifact.decoder)?;
        let tokenizer = Tokenizer::load(&artifact.tokens)?;
        config.sos_id = tokenizer.sos_id();
        config.eos_id = tokenizer.eos_id();
        config.pad_id = tokenizer.pad_id();
        config.beam_size = config.beam_size.max(1);
        config.max_decode_len = config.max_decode_len.max(1);

        if let Some(output) = artifact.decoder.outputs.first() {
            if let Some(&vocabulary) = output.shape.last() {
                if vocabulary > 0 && vocabulary as usize != tokenizer.vocab_size() {
                    bail!(
                        "解码器词表大小 {} 与 tokens.txt {} 不一致",
                        vocabulary,
                        tokenizer.vocab_size()
                    );
                }
            }
        }
        Ok(Self {
            encoder,
            decoder,
            tokenizer,
            config,
        })
    }

    pub fn transcribe(&mut self, features: &[Vec<f32>]) -> anyhow::Result<AsrOutput> {
        self.transcribe_with_beam(features, self.config.beam_size)
    }

    /// Runs the encoder once and decodes with an explicit beam size. Partial
    /// recognition uses a smaller beam because it runs while audio is arriving.
    pub fn transcribe_with_beam(
        &mut self,
        features: &[Vec<f32>],
        beam_size: usize,
    ) -> anyhow::Result<AsrOutput> {
        if features.is_empty() {
            return Ok(AsrOutput {
                text: String::new(),
                confidence: None,
            });
        }
        let audio_ms = features.len() as u64 * self.config.fbank.shift_ms as u64;
        if audio_ms > MAX_AUDIO_MS {
            bail!("语音段超过 {} ms 上限", MAX_AUDIO_MS);
        }

        let frame_count = features.len();
        let feature_dim = features[0].len();
        if features.iter().any(|frame| frame.len() != feature_dim) {
            bail!("fbank 帧维度不一致");
        }
        let mut flat = Vec::with_capacity(frame_count * feature_dim);
        for frame in features {
            flat.extend_from_slice(frame);
        }

        let mut inputs: Vec<(String, SessionInputValue<'_>)> =
            Vec::with_capacity(self.encoder.inputs().len());
        for input in self.encoder.inputs() {
            let name = input.name().to_string();
            match input.dtype() {
                ValueType::Tensor { ty, shape, .. } if *ty == TensorElementType::Float32 => {
                    if shape.len() != 3 {
                        bail!("ASR 编码器 f32 输入 {} 必须是三维特征张量", name);
                    }
                    let tensor = Tensor::from_array((
                        [1usize, frame_count, feature_dim],
                        flat.clone().into_boxed_slice(),
                    ))?;
                    inputs.push((name, tensor.into_dyn().into()));
                }
                ValueType::Tensor { ty, .. } if *ty == TensorElementType::Int64 => {
                    let lengths = Tensor::from_array((
                        [1usize],
                        vec![frame_count as i64].into_boxed_slice(),
                    ))?;
                    inputs.push((name, lengths.into_dyn().into()));
                }
                other => bail!("ASR 编码器不支持输入 {}：{other:?}", name),
            }
        }

        let outputs = self.encoder.run(inputs).context("ASR 编码器推理失败")?;
        let output = &outputs[0];
        let (shape, data) = output
            .try_extract_tensor::<f32>()
            .context("ASR 编码器输出类型错误")?;
        let shape: Vec<usize> = shape
            .iter()
            .map(|&dim| if dim <= 0 { 1 } else { dim as usize })
            .collect();
        let encoder_out = Tensor::from_array((shape, data.to_vec().into_boxed_slice()))
            .context("构造 encoder_out 张量失败")?;

        let mut decode_config = self.config.clone();
        decode_config.beam_size = beam_size.max(1);
        let hypotheses = beam_search(&mut self.decoder, &encoder_out, &decode_config)?;
        let Some(hypothesis) = hypotheses.first() else {
            return Ok(AsrOutput {
                text: String::new(),
                confidence: None,
            });
        };
        Ok(AsrOutput {
            text: self.tokenizer.decode(&hypothesis.tokens),
            confidence: confidence_of(hypothesis),
        })
    }
}

fn confidence_of(hypothesis: &Hypothesis) -> Option<f32> {
    if hypothesis.tokens.is_empty() {
        None
    } else {
        Some((hypothesis.score / hypothesis.tokens.len() as f32).exp())
    }
}
