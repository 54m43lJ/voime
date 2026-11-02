use std::io::{stdin, stdout};

use anyhow::{bail, Context};

use crate::aed::FireRedAed;
use crate::cli::Args;
use crate::protocol::{self, ErrorCode, Request, Response, PROTOCOL_VERSION};
use crate::runtime::GpuRuntime;

pub struct Server {
    runtime: GpuRuntime,
    aed: Option<FireRedAed>,
    verbose: bool,
}

impl Server {
    pub fn run(args: Args) -> anyhow::Result<()> {
        let runtime = GpuRuntime::load(&args.device).map_err(|error| {
            let _ = write_error(
                &mut stdout(),
                None,
                ErrorCode::RuntimeMissing,
                &format!("{error:#}"),
            );
            error
        })?;
        let mut server = Self {
            runtime,
            aed: None,
            verbose: args.verbose,
        };
        if args.verbose {
            eprintln!(
                "voime-gpu: provider={} device={}",
                server.runtime.provider(),
                server.runtime.device()
            );
        }
        server.serve()
    }

    fn serve(&mut self) -> anyhow::Result<()> {
        let mut input = stdin();
        let mut output = stdout();
        loop {
            let frame = match protocol::read_frame(&mut input).context("读取 worker 请求失败")?
            {
                Some(frame) => frame,
                None => return Ok(()),
            };
            let request: Request =
                protocol::parse_header(&frame).context("解析 worker 请求失败")?;
            match request {
                Request::Hello {
                    protocol,
                    manifest,
                    device,
                } => {
                    if protocol != PROTOCOL_VERSION {
                        write_error(
                            &mut output,
                            None,
                            ErrorCode::ProtocolMismatch,
                            &format!("worker 协议 {PROTOCOL_VERSION} 与 daemon {protocol} 不一致"),
                        )?;
                        bail!("worker 协议版本不匹配");
                    }
                    let runtime = match GpuRuntime::load(&device) {
                        Ok(runtime) => runtime,
                        Err(error) => {
                            write_error(
                                &mut output,
                                None,
                                ErrorCode::DeviceUnavailable,
                                &format!("{error:#}"),
                            )?;
                            bail!("重新加载 GPU runtime 失败：{error:#}");
                        }
                    };
                    self.runtime = runtime;
                    match FireRedAed::load(std::path::Path::new(&manifest), &self.runtime) {
                        Ok(aed) => {
                            self.aed = Some(aed);
                            protocol::write_frame(
                                &mut output,
                                &Response::Ready {
                                    protocol: PROTOCOL_VERSION,
                                    backend: "fire-red-aed".to_string(),
                                    provider: self.runtime.provider().to_string(),
                                    device: self.runtime.device().to_string(),
                                    vram_mb: self.runtime.vram_mb(),
                                    supports_partial: false,
                                },
                                &[],
                            )?;
                        }
                        Err(error) => {
                            write_error(
                                &mut output,
                                None,
                                ErrorCode::LoadFailed,
                                &format!("{error:#}"),
                            )?;
                            bail!("加载 FireRed AED 失败：{error:#}");
                        }
                    }
                }
                Request::Transcribe { id, .. } => {
                    let Some(aed) = self.aed.as_mut() else {
                        write_error(
                            &mut output,
                            Some(id),
                            ErrorCode::LoadFailed,
                            "worker 尚未完成握手",
                        )?;
                        continue;
                    };
                    if self.verbose {
                        eprintln!(
                            "voime-gpu: Transcribe id={id} samples={}",
                            frame.audio.len()
                        );
                    }
                    match aed.transcribe(&frame.audio) {
                        Ok(result) => {
                            if self.verbose {
                                eprintln!(
                                    "voime-gpu: Result id={id} chars={} latency={}ms rtf={:.3}",
                                    result.text.chars().count(),
                                    result.latency_ms,
                                    result.rtf
                                );
                            }
                            protocol::write_frame(
                                &mut output,
                                &Response::Result {
                                    id,
                                    text: result.text,
                                    latency_ms: result.latency_ms,
                                    rtf: result.rtf,
                                },
                                &[],
                            )?;
                        }
                        Err(error) => {
                            write_error(
                                &mut output,
                                Some(id),
                                ErrorCode::InferenceFailed,
                                &format!("{error:#}"),
                            )?;
                        }
                    }
                }
                Request::Cancel { id } => {
                    write_error(
                        &mut output,
                        Some(id),
                        ErrorCode::Timeout,
                        "请求已取消或当前没有可取消的请求",
                    )?;
                }
                Request::Shutdown => {
                    protocol::write_frame(&mut output, &Response::Bye, &[])?;
                    self.aed.take();
                    return Ok(());
                }
            }
        }
    }
}

fn write_error(
    writer: &mut impl std::io::Write,
    id: Option<u64>,
    code: ErrorCode,
    message: &str,
) -> anyhow::Result<()> {
    protocol::write_frame(
        writer,
        &Response::Error {
            id,
            code: code.as_str().to_string(),
            message: message.to_string(),
        },
        &[],
    )
}
