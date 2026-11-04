use std::path::Path;

use anyhow::{bail, Context};

pub struct Cmvn {
    pub mean: Vec<f32>,
    pub inv_stddev: Vec<f32>,
}

impl Cmvn {
    pub fn load(path: &Path) -> anyhow::Result<Self> {
        let bytes = std::fs::read(path)
            .with_context(|| format!("无法读取 CMVN 文件 {}", path.display()))?;
        let values = parse_kaldi_values(&bytes)
            .with_context(|| format!("解析 CMVN 文件失败 {}", path.display()))?;
        let (mean, inv_stddev) = cmvn_from_stats(&values, 80)
            .with_context(|| format!("CMVN 维度非法 {}", path.display()))?;
        Ok(Self { mean, inv_stddev })
    }

    pub fn apply(&self, features: &mut [Vec<f32>]) -> anyhow::Result<()> {
        for (index, frame) in features.iter_mut().enumerate() {
            if frame.len() != self.mean.len() {
                bail!(
                    "CMVN 帧 {} 维度 {}，期望 {}",
                    index,
                    frame.len(),
                    self.mean.len()
                );
            }
            for (bin, value) in frame.iter_mut().enumerate() {
                *value = (*value - self.mean[bin]) * self.inv_stddev[bin];
            }
        }
        Ok(())
    }
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
    if values.len() == 3 * dim && values[0] > 0.0 {
        return stats_to_cmvn(values[0] as f64, &values[dim..2 * dim], &values[2 * dim..]);
    }
    if values.len() == 2 * dim + 1 && values[0] > 0.0 {
        return stats_to_cmvn(values[0] as f64, &values[1..1 + dim], &values[1 + dim..]);
    }
    if values.len() == 2 * dim {
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
) -> anyhow::Result<(Vec<f32>, Vec<f32>)> {
    let dim = sums.len();
    if squares.len() != dim {
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
