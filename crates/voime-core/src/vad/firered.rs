use std::path::Path;

use anyhow::{bail, Context};
use ort::session::Session;
use ort::session::SessionInputValue;
use ort::value::Tensor;

use crate::asr::features::{apply_cmvn, FbankConfig, StreamingFbank};
use crate::infer::session::{
    dtype_is_f32, dtype_is_i32, dtype_is_i64, input_specs, output_specs, SessionFactory, TensorSpec,
};
use crate::model::ModelManifest;

pub const FRAME_SHIFT_MS: u32 = 10;

#[derive(Clone, Copy, Debug)]
pub struct VadConfig {
    pub speech_threshold: f32,
    pub fbank: FbankConfig,
}

impl Default for VadConfig {
    fn default() -> Self {
        Self {
            speech_threshold: 0.5,
            fbank: FbankConfig::default(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct VadFrame {
    pub speech_prob: f32,
    pub is_speech: bool,
}

enum StateValue {
    F32(Vec<f32>),
    I64(Vec<i64>),
    I32(Vec<i32>),
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
    cmvn: Option<(Vec<f32>, Vec<f32>)>,
    config: VadConfig,
}

impl FireRedVad {
    pub fn new(
        manifest: &ModelManifest,
        factory: &SessionFactory,
        config: VadConfig,
    ) -> anyhow::Result<Self> {
        let session = factory.build(manifest)?;
        let inputs = input_specs(&session)?;
        let outputs = output_specs(&session)?;
        let feature = inputs
            .iter()
            .find(|input| {
                input.shape.len() == 3
                    && input.shape[2] == config.fbank.num_bins as i64
                    && dtype_is_f32(&input.dtype)
            })
            .context("VAD 模型缺少 [1, T, num_bins] 的 fbank 输入")?;
        let probs_output = outputs
            .iter()
            .find(|output| {
                output.shape.len() >= 2
                    && *output.shape.last().unwrap_or(&0) == 1
                    && dtype_is_f32(&output.dtype)
            })
            .context("VAD 模型缺少逐帧概率输出")?
            .name
            .clone();

        let state_specs: Vec<&TensorSpec> = inputs
            .iter()
            .filter(|input| input.name != feature.name)
            .collect();
        let state_output_specs: Vec<TensorSpec> = outputs
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

        let cmvn = match manifest.file("cmvn.ark") {
            Some(path) => Some(load_vad_cmvn(path, config.fbank.num_bins)?),
            None => None,
        };

        Ok(Self {
            session,
            feature_name: feature.name.clone(),
            probs_output,
            states,
            state_output_specs,
            fbank: StreamingFbank::new(config.fbank),
            cmvn,
            config,
        })
    }

    /// Feeds a batch of 16 kHz mono samples and returns one frame per 10 ms.
    pub fn push(&mut self, samples_16k: &[f32]) -> anyhow::Result<Vec<VadFrame>> {
        let mut frames = self.fbank.push(samples_16k)?;
        if frames.is_empty() {
            return Ok(Vec::new());
        }
        if let Some((mean, inv_stddev)) = &self.cmvn {
            apply_cmvn(&mut frames, mean, inv_stddev).context("应用 VAD CMVN 失败")?;
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
                StateValue::I32(data) => Tensor::from_array((
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
                StateValue::I32(_) => {
                    let (_shape, data) = output
                        .try_extract_tensor::<i32>()
                        .context("VAD 状态输出类型错误")?;
                    StateValue::I32(data.to_vec())
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
                StateValue::I32(data) => StateValue::I32(vec![0; data.len()]),
            };
        }
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
    let value = if dtype_is_f32(&spec.dtype) {
        StateValue::F32(vec![0.0; count])
    } else if dtype_is_i64(&spec.dtype) {
        StateValue::I64(vec![0; count])
    } else if dtype_is_i32(&spec.dtype) {
        StateValue::I32(vec![0; count])
    } else {
        bail!("VAD 状态张量 {} 类型不支持：{}", spec.name, spec.dtype);
    };
    Ok(StateTensor {
        name: spec.name.clone(),
        input_shape: spec.shape.clone(),
        value,
    })
}

fn load_vad_cmvn(path: &Path, dim: usize) -> anyhow::Result<(Vec<f32>, Vec<f32>)> {
    let bytes = std::fs::read(path)
        .with_context(|| format!("读取 VAD CMVN 文件失败 {}", path.display()))?;
    let values = parse_kaldi_values(&bytes)
        .with_context(|| format!("解析 VAD CMVN 文件失败 {}", path.display()))?;
    cmvn_from_stats(&values, dim).with_context(|| format!("VAD CMVN 维度不匹配 {}", path.display()))
}

fn parse_kaldi_values(bytes: &[u8]) -> anyhow::Result<Vec<f32>> {
    let data = bytes.strip_prefix(b"\0B").unwrap_or(bytes);
    if let Some((token, mut rest)) = read_token(data) {
        match token.as_str() {
            "FM" => return read_f32_matrix(&mut rest),
            "DM" => return read_f64_matrix_as_f32(&mut rest),
            "F" => return read_f32_vector(&mut rest),
            "D" => return read_f64_vector_as_f32(&mut rest),
            _ => {}
        }
    }
    let text = String::from_utf8_lossy(data).replace(['[', ']'], " ");
    text.split_whitespace()
        .map(|value| {
            value
                .parse::<f32>()
                .with_context(|| format!("非法 CMVN 数值 {value}"))
        })
        .collect()
}

fn read_token(data: &[u8]) -> Option<(String, &[u8])> {
    let trimmed = data.strip_prefix(b" ").unwrap_or(data);
    let end = trimmed.iter().position(|byte| *byte == b' ')?;
    let token = std::str::from_utf8(&trimmed[..end]).ok()?.to_string();
    Some((token, &trimmed[end + 1..]))
}

fn read_u32(data: &mut &[u8]) -> anyhow::Result<u32> {
    if data.len() < 4 {
        bail!("Kaldi CMVN 文件被截断");
    }
    let (head, tail) = data.split_at(4);
    *data = tail;
    Ok(u32::from_le_bytes([head[0], head[1], head[2], head[3]]))
}

fn read_f32_matrix(data: &mut &[u8]) -> anyhow::Result<Vec<f32>> {
    let rows = read_u32(data)? as usize;
    let cols = read_u32(data)? as usize;
    let count = rows.checked_mul(cols).context("CMVN 矩阵尺寸溢出")?;
    read_f32_values(data, count)
}

fn read_f64_matrix_as_f32(data: &mut &[u8]) -> anyhow::Result<Vec<f32>> {
    let rows = read_u32(data)? as usize;
    let cols = read_u32(data)? as usize;
    let count = rows.checked_mul(cols).context("CMVN 矩阵尺寸溢出")?;
    read_f64_values_as_f32(data, count)
}

fn read_f32_vector(data: &mut &[u8]) -> anyhow::Result<Vec<f32>> {
    let count = read_u32(data)? as usize;
    read_f32_values(data, count)
}

fn read_f64_vector_as_f32(data: &mut &[u8]) -> anyhow::Result<Vec<f32>> {
    let count = read_u32(data)? as usize;
    read_f64_values_as_f32(data, count)
}

fn read_f32_values(data: &mut &[u8], count: usize) -> anyhow::Result<Vec<f32>> {
    let bytes = count.checked_mul(4).context("CMVN 数据尺寸溢出")?;
    if data.len() < bytes {
        bail!("Kaldi CMVN 文件被截断");
    }
    let (head, tail) = data.split_at(bytes);
    *data = tail;
    Ok(head
        .chunks_exact(4)
        .map(|chunk| f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
        .collect())
}

fn read_f64_values_as_f32(data: &mut &[u8], count: usize) -> anyhow::Result<Vec<f32>> {
    let bytes = count.checked_mul(8).context("CMVN 数据尺寸溢出")?;
    if data.len() < bytes {
        bail!("Kaldi CMVN 文件被截断");
    }
    let (head, tail) = data.split_at(bytes);
    *data = tail;
    Ok(head
        .chunks_exact(8)
        .map(|chunk| {
            f64::from_le_bytes([
                chunk[0], chunk[1], chunk[2], chunk[3], chunk[4], chunk[5], chunk[6], chunk[7],
            ]) as f32
        })
        .collect())
}

fn cmvn_from_stats(values: &[f32], dim: usize) -> anyhow::Result<(Vec<f32>, Vec<f32>)> {
    if dim == 0 {
        bail!("CMVN 维度必须大于 0");
    }
    if values.len() == 3 * dim {
        let count = values[0] as f64;
        if count > 0.0 {
            return stats_to_cmvn(count, &values[dim..2 * dim], &values[2 * dim..], dim);
        }
    }
    if values.len() == 2 * dim + 1 {
        let count = values[0] as f64;
        if count > 0.0 {
            return stats_to_cmvn(count, &values[1..1 + dim], &values[1 + dim..], dim);
        }
    }
    if values.len() == 2 * dim {
        // Some Kaldi exports store precomputed mean / inverse stddev.
        return Ok((values[..dim].to_vec(), values[dim..].to_vec()));
    }
    if values.len() == dim {
        return Ok((values.to_vec(), vec![1.0; dim]));
    }
    bail!("CMVN 统计长度 {} 与维度 {dim} 不匹配", values.len())
}

fn stats_to_cmvn(
    count: f64,
    sums: &[f32],
    squares: &[f32],
    dim: usize,
) -> anyhow::Result<(Vec<f32>, Vec<f32>)> {
    if sums.len() != dim || squares.len() != dim {
        bail!("CMVN 统计维度不匹配");
    }
    let mut mean = Vec::with_capacity(dim);
    let mut inv_stddev = Vec::with_capacity(dim);
    for (&sum, &square) in sums.iter().zip(squares) {
        let mean_value = sum as f64 / count;
        let variance = (square as f64 / count - mean_value * mean_value).max(1e-8);
        mean.push(mean_value as f32);
        inv_stddev.push((1.0 / variance.sqrt()) as f32);
    }
    Ok((mean, inv_stddev))
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
