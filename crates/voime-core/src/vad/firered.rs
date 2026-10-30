use anyhow::{bail, Context};
use ort::session::Session;
use ort::session::SessionInputValue;
use ort::value::Tensor;

use crate::asr::features::{FbankConfig, StreamingFbank};
use crate::infer::session::SessionFactory;
use crate::model::{Artifact, TensorSpec};

pub const FRAME_SHIFT_MS: u32 = 10;

#[derive(Clone, Copy, Debug)]
pub struct VadConfig {
    pub speech_threshold: f32,
    pub min_speech_ms: u32,
    pub min_silence_ms: u32,
    pub fbank: FbankConfig,
}

impl Default for VadConfig {
    fn default() -> Self {
        Self {
            speech_threshold: 0.5,
            min_speech_ms: 80,
            min_silence_ms: 200,
            fbank: FbankConfig::default(),
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct VadFrame {
    pub speech_prob: f32,
    pub is_speech: bool,
}

enum StateValue {
    F32(Vec<f32>),
    I64(Vec<i64>),
}

struct StateTensor {
    name: String,
    input_shape: Vec<i64>,
    value: StateValue,
}

pub struct FireRedVad {
    session: Session,
    feature_name: String,
    probs_output: String,
    states: Vec<StateTensor>,
    state_output_specs: Vec<TensorSpec>,
    fbank: StreamingFbank,
    config: VadConfig,
}

impl FireRedVad {
    pub fn new(
        artifact: &Artifact,
        factory: &SessionFactory,
        config: VadConfig,
    ) -> anyhow::Result<Self> {
        let session = factory.build(artifact)?;
        let feature = artifact
            .inputs
            .iter()
            .find(|input| {
                input.shape.len() == 3
                    && input.shape[2] == config.fbank.num_bins as i64
                    && crate::infer::session::dtype_is_f32(&input.dtype)
            })
            .context("VAD 清单缺少 [1, T, num_bins] 的 fbank 输入")?;
        let probs_output = artifact
            .outputs
            .iter()
            .find(|output| {
                output.shape.len() >= 2
                    && *output.shape.last().unwrap_or(&0) == 1
                    && crate::infer::session::dtype_is_f32(&output.dtype)
            })
            .or_else(|| artifact.outputs.first())
            .context("VAD 清单缺少逐帧概率输出")?
            .name
            .clone();

        let state_specs: Vec<&TensorSpec> = artifact
            .inputs
            .iter()
            .filter(|input| input.name != feature.name)
            .collect();
        let state_output_specs: Vec<TensorSpec> = artifact
            .outputs
            .iter()
            .filter(|output| output.name != probs_output)
            .cloned()
            .collect();
        if state_specs.len() != state_output_specs.len() {
            bail!(
                "VAD 状态输入/输出数量不一致：{} vs {}",
                state_specs.len(),
                state_output_specs.len()
            );
        }
        let mut states = Vec::with_capacity(state_specs.len());
        for spec in state_specs {
            states.push(zero_state(spec)?);
        }

        Ok(Self {
            session,
            feature_name: feature.name.clone(),
            probs_output,
            states,
            state_output_specs,
            fbank: StreamingFbank::new(config.fbank),
            config,
        })
    }

    /// Feeds a batch of 16 kHz mono samples and returns one probability per
    /// 10 ms frame produced from this batch.
    pub fn push(&mut self, samples_16k: &[f32]) -> anyhow::Result<Vec<VadFrame>> {
        let frames = self.fbank.push(samples_16k)?;
        if frames.is_empty() {
            return Ok(Vec::new());
        }
        let frame_count = frames.len();
        let num_bins = self.config.fbank.num_bins;
        let mut flat = Vec::with_capacity(frame_count * num_bins);
        for frame in &frames {
            flat.extend_from_slice(frame);
        }
        let tensor = Tensor::from_array(([1usize, frame_count, num_bins], flat.into_boxed_slice()))
            .context("构造 VAD 输入张量失败")?;

        let mut inputs: Vec<(String, SessionInputValue<'_>)> =
            Vec::with_capacity(1 + self.states.len());
        inputs.push((self.feature_name.clone(), tensor.into_dyn().into()));
        for state in &self.states {
            let value = match &state.value {
                StateValue::F32(data) => Tensor::from_array((
                    shape_usize(&state.input_shape)?,
                    data.clone().into_boxed_slice(),
                ))?
                .into_dyn()
                .into(),
                StateValue::I64(data) => Tensor::from_array((
                    shape_usize(&state.input_shape)?,
                    data.clone().into_boxed_slice(),
                ))?
                .into_dyn()
                .into(),
            };
            inputs.push((state.name.clone(), value));
        }

        let outputs = self.session.run(inputs).context("VAD 推理失败")?;
        let probs_tensor = outputs
            .get(&self.probs_output)
            .with_context(|| format!("VAD 输出缺失：{}", self.probs_output))?;
        let (_shape, probs) = probs_tensor
            .try_extract_tensor::<f32>()
            .context("VAD 概率输出类型错误")?;
        if probs.len() != frame_count {
            bail!(
                "VAD 概率数量 {} 与特征帧数 {} 不一致",
                probs.len(),
                frame_count
            );
        }

        for (state, spec) in self.states.iter_mut().zip(&self.state_output_specs) {
            let output = outputs
                .get(&spec.name)
                .with_context(|| format!("VAD 状态输出缺失：{}", spec.name))?;
            state.value = match &state.value {
                StateValue::F32(_) => {
                    let (_shape, data) = output
                        .try_extract_tensor::<f32>()
                        .context("VAD 状态输出类型错误")?;
                    StateValue::F32(data.to_vec())
                }
                StateValue::I64(_) => {
                    let (_shape, data) = output
                        .try_extract_tensor::<i64>()
                        .context("VAD 状态输出类型错误")?;
                    StateValue::I64(data.to_vec())
                }
            };
        }

        Ok(probs
            .iter()
            .map(|&speech_prob| VadFrame {
                speech_prob,
                is_speech: speech_prob >= self.config.speech_threshold,
            })
            .collect())
    }

    pub fn reset(&mut self) {
        self.fbank = StreamingFbank::new(self.config.fbank);
        for state in &mut self.states {
            state.value = match &state.value {
                StateValue::F32(data) => StateValue::F32(vec![0.0; data.len()]),
                StateValue::I64(data) => StateValue::I64(vec![0; data.len()]),
            };
        }
    }

    pub fn min_speech_ms(&self) -> u32 {
        self.config.min_speech_ms
    }

    pub fn min_silence_ms(&self) -> u32 {
        self.config.min_silence_ms
    }
}

fn zero_state(spec: &TensorSpec) -> anyhow::Result<StateTensor> {
    if spec.shape.iter().any(|&dim| dim < 0) {
        bail!("VAD 状态张量 {} 必须声明固定形状", spec.name);
    }
    let count = spec
        .shape
        .iter()
        .try_fold(1usize, |product, &dim| product.checked_mul(dim as usize))
        .context("VAD 状态张量尺寸溢出")?;
    let value = if crate::infer::session::dtype_is_f32(&spec.dtype) {
        StateValue::F32(vec![0.0; count])
    } else if crate::infer::session::dtype_is_i64(&spec.dtype) {
        StateValue::I64(vec![0; count])
    } else {
        bail!("VAD 状态张量 {} 类型不支持：{}", spec.name, spec.dtype);
    };
    Ok(StateTensor {
        name: spec.name.clone(),
        input_shape: spec.shape.clone(),
        value,
    })
}

fn shape_usize(shape: &[i64]) -> anyhow::Result<Vec<usize>> {
    shape
        .iter()
        .map(|&dim| {
            if dim < 0 {
                bail!("动态形状不能用于状态张量");
            }
            Ok(dim as usize)
        })
        .collect()
}
