use std::io::{Read, Write};

use anyhow::{bail, Context};
use serde::{de::DeserializeOwned, Deserialize, Serialize};

pub const PROTOCOL_VERSION: u32 = 1;
pub const MAX_HEADER_BYTES: u32 = 1024 * 1024;
pub const MAX_AUDIO_BYTES: u32 = 64 * 1024 * 1024;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum Request {
    Hello {
        protocol: u32,
        manifest: String,
        device: String,
    },
    Transcribe {
        id: u64,
        sample_rate: u32,
        language: Option<String>,
        use_itn: bool,
    },
    Cancel {
        id: u64,
    },
    Shutdown,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum Response {
    Ready {
        protocol: u32,
        backend: String,
        provider: String,
        device: String,
        vram_mb: u32,
        supports_partial: bool,
    },
    Result {
        id: u64,
        text: String,
        latency_ms: u64,
        rtf: f32,
    },
    Error {
        #[serde(default)]
        id: Option<u64>,
        code: String,
        message: String,
    },
    Bye,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ErrorCode {
    RuntimeMissing,
    DeviceUnavailable,
    LoadFailed,
    InferenceFailed,
    Timeout,
    ProtocolMismatch,
}

impl ErrorCode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::RuntimeMissing => "RuntimeMissing",
            Self::DeviceUnavailable => "DeviceUnavailable",
            Self::LoadFailed => "LoadFailed",
            Self::InferenceFailed => "InferenceFailed",
            Self::Timeout => "Timeout",
            Self::ProtocolMismatch => "ProtocolMismatch",
        }
    }
}

#[derive(Clone, Debug)]
pub struct Frame {
    /// UTF-8 JSON header. The caller parses it as `Request` or `Response`.
    pub header: Vec<u8>,
    pub audio: Vec<f32>,
}

pub fn read_frame(reader: &mut impl Read) -> anyhow::Result<Option<Frame>> {
    let Some(header_len) = read_u32(reader)? else {
        return Ok(None);
    };
    if header_len == 0 || header_len > MAX_HEADER_BYTES {
        bail!("worker 头长度非法：{header_len}");
    }
    let mut header = vec![0u8; header_len as usize];
    reader
        .read_exact(&mut header)
        .context("读取 worker 头失败")?;
    let Some(audio_len) = read_u32(reader)? else {
        bail!("worker 帧缺少音频长度");
    };
    if audio_len > MAX_AUDIO_BYTES {
        bail!("worker 音频长度非法：{audio_len}");
    }
    if audio_len % 4 != 0 {
        bail!("worker 音频长度不是 f32 的倍数：{audio_len}");
    }
    let mut bytes = vec![0u8; audio_len as usize];
    reader
        .read_exact(&mut bytes)
        .context("读取 worker 音频失败")?;
    #[allow(clippy::chunks_exact_to_as_chunks)]
    let audio = bytes
        .chunks_exact(4)
        .map(|chunk| f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
        .collect();
    Ok(Some(Frame { header, audio }))
}

pub fn parse_header<T: DeserializeOwned>(frame: &Frame) -> anyhow::Result<T> {
    serde_json::from_slice(&frame.header).context("worker 头不是合法 JSON")
}

pub fn write_frame<W: Write, T: Serialize>(
    writer: &mut W,
    header: &T,
    audio: &[f32],
) -> anyhow::Result<()> {
    let header = serde_json::to_vec(header).context("序列化 worker 头失败")?;
    if header.len() > MAX_HEADER_BYTES as usize {
        bail!("worker 头超过 {MAX_HEADER_BYTES} 字节");
    }
    let audio_len = audio.len().saturating_mul(4);
    if audio_len > MAX_AUDIO_BYTES as usize {
        bail!("worker 音频超过 {MAX_AUDIO_BYTES} 字节");
    }
    writer
        .write_all(&(header.len() as u32).to_le_bytes())
        .context("写入 worker 头长度失败")?;
    writer.write_all(&header).context("写入 worker 头失败")?;
    writer
        .write_all(&(audio_len as u32).to_le_bytes())
        .context("写入 worker 音频长度失败")?;
    for sample in audio {
        writer
            .write_all(&sample.to_le_bytes())
            .context("写入 worker 音频失败")?;
    }
    writer.flush().context("刷新 worker 帧失败")?;
    Ok(())
}

fn read_u32(reader: &mut impl Read) -> anyhow::Result<Option<u32>> {
    let mut bytes = [0u8; 4];
    let mut read = 0usize;
    while read < bytes.len() {
        match reader.read(&mut bytes[read..]) {
            Ok(0) if read == 0 => return Ok(None),
            Ok(0) => bail!("worker 帧被截断"),
            Ok(count) => read += count,
            Err(error) => return Err(error).context("读取 worker 帧失败"),
        }
    }
    Ok(Some(u32::from_le_bytes(bytes)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_transcribe_frame() {
        let request = Request::Transcribe {
            id: 9,
            sample_rate: 16_000,
            language: Some("zh".to_string()),
            use_itn: true,
        };
        let audio = vec![0.0, 0.5, -0.5];
        let mut bytes = Vec::new();
        write_frame(&mut bytes, &request, &audio).unwrap();
        let mut cursor = std::io::Cursor::new(bytes);
        let frame = read_frame(&mut cursor).unwrap().unwrap();
        assert_eq!(frame.audio, audio);
        let decoded: Request = parse_header(&frame).unwrap();
        assert!(matches!(decoded, Request::Transcribe { id: 9, .. }));
    }

    #[test]
    fn rejects_oversized_audio() {
        let request = Request::Shutdown;
        let audio = vec![0.0; (MAX_AUDIO_BYTES as usize / 4) + 1];
        let mut bytes = Vec::new();
        assert!(write_frame(&mut bytes, &request, &audio).is_err());
    }
}
