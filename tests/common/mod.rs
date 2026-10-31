use std::path::Path;

pub fn read_wav(path: &Path) -> anyhow::Result<Vec<f32>> {
    let bytes = std::fs::read(path)?;
    if bytes.len() < 12 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        anyhow::bail!("不是 RIFF/WAVE 文件：{}", path.display());
    }
    let mut offset = 12usize;
    let mut format = None;
    let mut data = None;
    while offset + 8 <= bytes.len() {
        let id = &bytes[offset..offset + 4];
        let size = u32::from_le_bytes(bytes[offset + 4..offset + 8].try_into()?) as usize;
        let start = offset + 8;
        let end = start.saturating_add(size).min(bytes.len());
        match id {
            b"fmt " => format = Some(&bytes[start..end]),
            b"data" => data = Some(&bytes[start..end]),
            _ => {}
        }
        offset = end + (size & 1);
    }
    let format = format.ok_or_else(|| anyhow::anyhow!("WAV 缺少 fmt 块"))?;
    let data = data.ok_or_else(|| anyhow::anyhow!("WAV 缺少 data 块"))?;
    let channels = u16::from_le_bytes(format[2..4].try_into()?) as usize;
    let sample_rate = u32::from_le_bytes(format[4..8].try_into()?);
    let bits = u16::from_le_bytes(format[14..16].try_into()?) as usize;
    anyhow::ensure!(sample_rate == 16_000, "采样率不是 16 kHz");
    anyhow::ensure!(channels == 1, "测试音频不是单声道");
    anyhow::ensure!(bits == 16, "测试音频不是 PCM16");
    Ok(data
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| i16::from_le_bytes(*pair) as f32 / 32768.0)
        .collect())
}
