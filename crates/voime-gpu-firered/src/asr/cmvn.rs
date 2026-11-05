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
            // Kaldi 的二进制向量 token 是 FV / DV；F / D 为旧版本兼容。
            "FV" | "F" => return read_f32_vector(&mut rest),
            "DV" | "D" => return read_f64_vector_as_f32(&mut rest),
            _ => {}
        }
    }
    parse_text_values(data)
}

fn parse_text_values(data: &[u8]) -> anyhow::Result<Vec<f32>> {
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

fn read_i32(data: &mut &[u8]) -> anyhow::Result<i32> {
    // Kaldi 二进制整数先写 1 字节宽度标记（有符号为正），再写原始小端数据。
    let Some((&size, _)) = data.split_first() else {
        bail!("Kaldi CMVN 文件被截断");
    };
    let width = (size as i8).unsigned_abs() as usize;
    if width != 4 && width != 8 {
        bail!("Kaldi CMVN 整数宽度不支持：{}", size as i8);
    }
    if data.len() < 1 + width {
        bail!("Kaldi CMVN 文件被截断");
    }
    let raw = &data[1..1 + width];
    let value = if width == 4 {
        i32::from_le_bytes(raw.try_into().expect("4 字节整数")) as i64
    } else {
        i64::from_le_bytes(raw.try_into().expect("8 字节整数"))
    };
    *data = &data[1 + width..];
    i32::try_from(value).context("Kaldi CMVN 整数溢出")
}

fn read_matrix_count(data: &mut &[u8]) -> anyhow::Result<usize> {
    let rows = read_i32(data)?;
    let cols = read_i32(data)?;
    if rows < 0 || cols < 0 {
        bail!("Kaldi CMVN 矩阵尺寸非法：{rows} x {cols}");
    }
    (rows as usize)
        .checked_mul(cols as usize)
        .context("CMVN 矩阵尺寸溢出")
}

fn read_vector_count(data: &mut &[u8]) -> anyhow::Result<usize> {
    let count = read_i32(data)?;
    if count < 0 {
        bail!("Kaldi CMVN 向量长度非法：{count}");
    }
    Ok(count as usize)
}

fn read_f32_matrix(data: &mut &[u8]) -> anyhow::Result<Vec<f32>> {
    let count = read_matrix_count(data)?;
    read_f32_values(data, count)
}

fn read_f64_matrix_as_f32(data: &mut &[u8]) -> anyhow::Result<Vec<f32>> {
    let count = read_matrix_count(data)?;
    read_f64_values_as_f32(data, count)
}

fn read_f32_vector(data: &mut &[u8]) -> anyhow::Result<Vec<f32>> {
    let count = read_vector_count(data)?;
    read_f32_values(data, count)
}

fn read_f64_vector_as_f32(data: &mut &[u8]) -> anyhow::Result<Vec<f32>> {
    let count = read_vector_count(data)?;
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
        .as_chunks::<4>()
        .0
        .iter()
        .map(|chunk| f32::from_le_bytes(*chunk))
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
        .as_chunks::<8>()
        .0
        .iter()
        .map(|chunk| f64::from_le_bytes(*chunk) as f32)
        .collect())
}

fn cmvn_from_stats(values: &[f32], dim: usize) -> anyhow::Result<(Vec<f32>, Vec<f32>)> {
    if dim == 0 {
        bail!("CMVN 维度必须大于 0");
    }
    // FireRed / Kaldi `compute-cmvn-stats` 的 2 x (dim + 1) 布局：
    // 第一行前 dim 列为 sum，最后一列为 count；第二行前 dim 列为平方和。
    if values.len() == 2 * (dim + 1) {
        let count = values[dim] as f64;
        if count <= 0.0 {
            bail!("CMVN count 非法：{count}");
        }
        return stats_to_cmvn(count, &values[..dim], &values[dim + 1..dim + 1 + dim]);
    }
    // 以下为旧导出的扁平布局，保留兼容。
    if values.len() == 3 * dim {
        let count = values[0] as f64;
        if count <= 0.0 {
            bail!("CMVN count 非法：{count}");
        }
        return stats_to_cmvn(count, &values[dim..2 * dim], &values[2 * dim..]);
    }
    if values.len() == 2 * dim + 1 {
        let count = values[0] as f64;
        if count <= 0.0 {
            bail!("CMVN count 非法：{count}");
        }
        return stats_to_cmvn(count, &values[1..1 + dim], &values[1 + dim..]);
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
        let variance = (square as f64 / count - mean_value * mean_value).max(1e-20);
        mean.push(mean_value as f32);
        inv_stddev.push((1.0 / variance.sqrt()) as f32);
    }
    Ok((mean, inv_stddev))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn binary_header(token: &str, rows: i32, cols: i32) -> Vec<u8> {
        let mut bytes = vec![0, b'B'];
        bytes.extend_from_slice(token.as_bytes());
        bytes.push(b' ');
        bytes.push(4);
        bytes.extend_from_slice(&rows.to_le_bytes());
        bytes.push(4);
        bytes.extend_from_slice(&cols.to_le_bytes());
        bytes
    }

    fn kaldi_f64_matrix(token: &str, rows: i32, cols: i32, values: &[f64]) -> Vec<u8> {
        let mut bytes = binary_header(token, rows, cols);
        for value in values {
            bytes.extend_from_slice(&value.to_le_bytes());
        }
        bytes
    }

    fn kaldi_f32_matrix(token: &str, rows: i32, cols: i32, values: &[f32]) -> Vec<u8> {
        let mut bytes = binary_header(token, rows, cols);
        for value in values {
            bytes.extend_from_slice(&value.to_le_bytes());
        }
        bytes
    }

    fn assert_close(actual: &[f32], expected: &[f32], tolerance: f32) {
        assert_eq!(actual.len(), expected.len());
        for (index, (actual, expected)) in actual.iter().zip(expected).enumerate() {
            assert!(
                (actual - expected).abs() <= tolerance,
                "第 {index} 项：{actual} != {expected}"
            );
        }
    }

    #[test]
    fn parses_firered_kaldi_dm_cmvn() {
        let dim = 3usize;
        let count = 1000.0f64;
        let means = [1.0f64, 2.0, 3.0];
        let variances = [4.0f64, 9.0, 16.0];
        let mut values: Vec<f64> = means.iter().map(|mean| mean * count).collect();
        values.push(count);
        values.extend(
            means
                .iter()
                .zip(variances)
                .map(|(mean, variance)| (variance + mean * mean) * count),
        );
        values.push(0.0);

        let bytes = kaldi_f64_matrix("DM", 2, dim as i32 + 1, &values);
        let parsed = parse_kaldi_values(&bytes).unwrap();
        let (mean, inv_stddev) = cmvn_from_stats(&parsed, dim).unwrap();
        assert_close(&mean, &[1.0, 2.0, 3.0], 1e-5);
        assert_close(&inv_stddev, &[0.5, 1.0 / 3.0, 0.25], 1e-5);
    }

    #[test]
    fn parses_kaldi_fm_matrix() {
        let bytes = kaldi_f32_matrix("FM", 2, 3, &[1.0, 2.0, 3.0, 4.0, 5.0, 6.0]);
        let parsed = parse_kaldi_values(&bytes).unwrap();
        assert_eq!(parsed, vec![1.0, 2.0, 3.0, 4.0, 5.0, 6.0]);
    }

    #[test]
    fn parses_kaldi_dv_vector() {
        let mut bytes = vec![0, b'B'];
        bytes.extend_from_slice(b"DV ");
        bytes.push(4);
        bytes.extend_from_slice(&2i32.to_le_bytes());
        for value in [1.5f64, -2.5] {
            bytes.extend_from_slice(&value.to_le_bytes());
        }
        let parsed = parse_kaldi_values(&bytes).unwrap();
        assert_eq!(parsed, vec![1.5, -2.5]);
    }

    #[test]
    fn parses_text_cmvn() {
        let parsed = parse_kaldi_values(b" [ 1 2 3 ]\n").unwrap();
        assert_eq!(parsed, vec![1.0, 2.0, 3.0]);
        let (mean, inv_stddev) = cmvn_from_stats(&parsed, 3).unwrap();
        assert_eq!(mean, vec![1.0, 2.0, 3.0]);
        assert_eq!(inv_stddev, vec![1.0, 1.0, 1.0]);
    }

    #[test]
    fn rejects_truncated_kaldi_matrix() {
        let mut bytes = kaldi_f64_matrix("DM", 2, 4, &[0.0; 8]);
        bytes.truncate(bytes.len() - 4);
        let error = parse_kaldi_values(&bytes).unwrap_err().to_string();
        assert!(error.contains("截断"), "unexpected error: {error}");
    }
}
