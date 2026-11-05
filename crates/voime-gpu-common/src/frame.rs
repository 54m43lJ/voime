use std::io::{Read, Write};

use anyhow::{bail, Context};

use crate::message::{Request, Response};
use crate::{read_exact_or_eof, MAX_AUDIO_BYTES, MAX_HEADER_BYTES};

#[derive(Clone, Debug, PartialEq)]
pub struct Frame {
    pub header: String,
    pub audio: Vec<f32>,
}

impl Frame {
    pub fn from_request(request: &Request, audio: &[f32]) -> anyhow::Result<Self> {
        if matches!(request, Request::Transcribe { .. }) {
            if audio.is_empty() {
                bail!("Transcribe 必须携带音频");
            }
        } else if !audio.is_empty() {
            bail!("非 Transcribe 请求不得携带音频");
        }
        Ok(Self {
            header: serde_json::to_string(request).context("序列化 GPU 请求失败")?,
            audio: audio.to_vec(),
        })
    }

    pub fn from_response(response: &Response) -> anyhow::Result<Self> {
        Ok(Self {
            header: serde_json::to_string(response).context("序列化 GPU 响应失败")?,
            audio: Vec::new(),
        })
    }

    pub fn request(&self) -> anyhow::Result<Request> {
        let request: Request =
            serde_json::from_str(&self.header).context("GPU 请求 header 不是合法 JSON")?;
        match request {
            Request::Transcribe { .. } if self.audio.is_empty() => {
                bail!("Transcribe 帧缺少音频")
            }
            Request::Transcribe { .. } => {}
            _ if !self.audio.is_empty() => bail!("非 Transcribe 请求携带了音频"),
            _ => {}
        }
        Ok(request)
    }

    pub fn response(&self) -> anyhow::Result<Response> {
        if !self.audio.is_empty() {
            bail!("GPU 响应不得携带音频");
        }
        serde_json::from_str(&self.header).context("GPU 响应 header 不是合法 JSON")
    }
}

pub fn read_frame<R: Read>(reader: &mut R) -> anyhow::Result<Option<Frame>> {
    let mut header_len_bytes = [0u8; 4];
    if !read_exact_or_eof(reader, &mut header_len_bytes)? {
        return Ok(None);
    }
    let header_len = u32::from_le_bytes(header_len_bytes) as usize;
    if header_len == 0 || header_len > MAX_HEADER_BYTES {
        bail!("GPU 帧 header 长度非法：{header_len}");
    }
    let mut header = vec![0u8; header_len];
    read_exact_or_eof(reader, &mut header)?
        .then_some(())
        .context("GPU 帧 header 截断")?;
    let header = String::from_utf8(header).context("GPU 帧 header 不是 UTF-8")?;

    let mut audio_len_bytes = [0u8; 4];
    read_exact_or_eof(reader, &mut audio_len_bytes)?
        .then_some(())
        .context("GPU 帧音频长度截断")?;
    let audio_len = u32::from_le_bytes(audio_len_bytes) as usize;
    if !audio_len.is_multiple_of(4) {
        bail!("GPU 帧音频长度不是 4 的倍数：{audio_len}");
    }
    if audio_len > MAX_AUDIO_BYTES {
        bail!("GPU 帧音频超过 {MAX_AUDIO_BYTES} 字节");
    }
    let mut audio_bytes = vec![0u8; audio_len];
    if audio_len > 0 {
        read_exact_or_eof(reader, &mut audio_bytes)?
            .then_some(())
            .context("GPU 帧音频截断")?;
    }
    let audio = audio_bytes
        .as_chunks::<4>()
        .0
        .iter()
        .map(|bytes| f32::from_le_bytes(*bytes))
        .collect();
    Ok(Some(Frame { header, audio }))
}

pub fn write_frame<W: Write>(writer: &mut W, frame: &Frame) -> anyhow::Result<()> {
    if frame.header.is_empty() || frame.header.len() > MAX_HEADER_BYTES {
        bail!("GPU 帧 header 长度非法：{}", frame.header.len());
    }
    let audio_len = frame.audio.len().saturating_mul(4);
    if audio_len > MAX_AUDIO_BYTES {
        bail!("GPU 帧音频超过 {MAX_AUDIO_BYTES} 字节");
    }
    writer.write_all(&(frame.header.len() as u32).to_le_bytes())?;
    writer.write_all(frame.header.as_bytes())?;
    writer.write_all(&(audio_len as u32).to_le_bytes())?;
    for sample in &frame.audio {
        writer.write_all(&sample.to_le_bytes())?;
    }
    writer.flush()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::message::{ErrorCode, GpuBackend};
    use std::io::Cursor;

    #[test]
    fn round_trips_transcribe_with_audio() {
        let request = Request::Transcribe {
            id: 42,
            sample_rate: 16_000,
            language: "zh".to_string(),
        };
        let frame = Frame::from_request(&request, &[0.0, 0.5, -0.5]).unwrap();
        let mut bytes = Vec::new();
        write_frame(&mut bytes, &frame).unwrap();
        let mut reader = Cursor::new(bytes);
        let decoded = read_frame(&mut reader).unwrap().unwrap();
        assert_eq!(decoded.audio, vec![0.0, 0.5, -0.5]);
        assert_eq!(decoded.request().unwrap(), request);
    }

    #[test]
    fn unit_messages_are_json_strings() {
        let frame = Frame::from_request(&Request::Shutdown, &[]).unwrap();
        assert_eq!(frame.header, "\"Shutdown\"");
        let ready = Response::Bye;
        assert_eq!(Frame::from_response(&ready).unwrap().header, "\"Bye\"");
    }

    #[test]
    fn clean_eof_and_truncated_frames() {
        let mut empty = Cursor::new(Vec::<u8>::new());
        assert!(read_frame(&mut empty).unwrap().is_none());
        let mut truncated = Cursor::new(vec![5u8, 0, 0, 0, b'{']);
        assert!(read_frame(&mut truncated).is_err());
    }

    #[test]
    fn rejects_wrong_audio_presence() {
        let frame = Frame {
            header: serde_json::to_string(&Request::Cancel { id: 1 }).unwrap(),
            audio: vec![0.0],
        };
        assert!(frame.request().is_err());
        let frame = Frame {
            header: serde_json::to_string(&Response::Error {
                id: 0,
                code: ErrorCode::LoadFailed,
                message: "x".to_string(),
            })
            .unwrap(),
            audio: vec![0.0],
        };
        assert!(frame.response().is_err());
        let _ = GpuBackend::Firered;
    }
}
