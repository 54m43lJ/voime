use std::io::{Read, Write};

use anyhow::{bail, Context};

use crate::frame::{read_frame, write_frame, Frame};
use crate::message::{ErrorCode, GpuBackend, Request, Response};
use crate::GPU_PROTOCOL_VERSION;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReadyInfo {
    pub backend: GpuBackend,
    pub runtime: String,
    pub device: String,
    pub vram_mb: u32,
    pub punc: bool,
}

pub fn handshake<R: Read, W: Write>(
    reader: &mut R,
    writer: &mut W,
    hello: &Request,
) -> anyhow::Result<ReadyInfo> {
    if !matches!(hello, Request::Hello { .. }) {
        bail!("handshake 需要 Hello 请求");
    }
    write_frame(writer, &Frame::from_request(hello, &[])?)?;
    let frame = read_frame(reader)?.context("worker 在 Ready 前关闭了连接")?;
    match frame.response()? {
        Response::Ready {
            protocol,
            backend,
            runtime,
            device,
            vram_mb,
            punc,
        } => {
            if protocol != GPU_PROTOCOL_VERSION {
                bail!("GPU 协议版本不匹配：daemon {GPU_PROTOCOL_VERSION}，worker {protocol}");
            }
            if let Request::Hello {
                backend: requested, ..
            } = hello
            {
                if requested != &backend {
                    bail!("worker 后端不匹配：请求 {requested}，实际 {backend}");
                }
            }
            Ok(ReadyInfo {
                backend,
                runtime,
                device,
                vram_mb,
                punc,
            })
        }
        Response::Error { code, message, .. } => {
            bail!("GPU worker 加载失败（{code}）：{message}")
        }
        _ => bail!("GPU worker 握手响应非法"),
    }
}

pub fn expect_result(response: Response, id: u64) -> anyhow::Result<Response> {
    match &response {
        Response::Result {
            id: response_id, ..
        }
        | Response::Error {
            id: response_id, ..
        } => {
            if *response_id != id {
                bail!("GPU 响应 id 不匹配：期望 {id}，实际 {response_id}");
            }
            Ok(response)
        }
        _ => bail!("GPU 响应不是 Transcribe 终态"),
    }
}

pub fn error_response(id: u64, code: ErrorCode, message: impl Into<String>) -> Response {
    let mut message = message.into();
    message = message.replace(['\r', '\n'], " ");
    if message.len() > 1024 {
        let mut end = 1024;
        while !message.is_char_boundary(end) {
            end -= 1;
        }
        message.truncate(end);
    }
    Response::Error { id, code, message }
}
