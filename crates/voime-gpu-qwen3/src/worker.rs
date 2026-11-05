use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Context;
use crossbeam_channel::Sender;
use voime_gpu_common::{
    error_response, read_frame, write_frame, ErrorCode, Frame, GpuBackend, Request, Response,
    GPU_PROTOCOL_VERSION,
};

use crate::engine::Engine;
use crate::manifest::{Manifest, SAMPLE_RATE};

pub const TRANSCRIBE_BUDGET_MS: u64 = 60_000;

#[derive(Clone)]
pub struct CancelFlag {
    flag: Arc<AtomicBool>,
    current: Arc<AtomicU64>,
    in_flight: Arc<AtomicBool>,
    shutdown: Arc<AtomicBool>,
}

impl CancelFlag {
    pub fn new() -> Self {
        Self {
            flag: Arc::new(AtomicBool::new(false)),
            current: Arc::new(AtomicU64::new(0)),
            in_flight: Arc::new(AtomicBool::new(false)),
            shutdown: Arc::new(AtomicBool::new(false)),
        }
    }

    pub fn arm(&self, id: u64) {
        self.current.store(id, Ordering::Relaxed);
        self.flag.store(false, Ordering::Relaxed);
        self.in_flight.store(true, Ordering::Relaxed);
    }

    pub fn cancel(&self, id: u64) {
        if self.current.load(Ordering::Relaxed) == id && self.in_flight.load(Ordering::Relaxed) {
            self.flag.store(true, Ordering::Relaxed);
        }
    }

    pub fn is_cancelled(&self) -> bool {
        self.flag.load(Ordering::Relaxed)
    }

    pub fn is_shutdown(&self) -> bool {
        self.shutdown.load(Ordering::Relaxed)
    }

    pub fn request_shutdown(&self) {
        self.shutdown.store(true, Ordering::Relaxed);
        self.flag.store(true, Ordering::Relaxed);
    }

    pub fn in_flight(&self) -> bool {
        self.in_flight.load(Ordering::Relaxed)
    }

    pub fn clear(&self) {
        self.flag.store(false, Ordering::Relaxed);
        self.current.store(0, Ordering::Relaxed);
        self.in_flight.store(false, Ordering::Relaxed);
    }
}

impl Default for CancelFlag {
    fn default() -> Self {
        Self::new()
    }
}

pub enum WorkerCommand {
    Transcribe {
        id: u64,
        language: String,
        audio: Vec<f32>,
    },
    Reject(Response),
    Shutdown,
    ProtocolError(String),
    Abort,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RunExit {
    Shutdown,
    Abort,
}

#[derive(Debug, thiserror::Error)]
pub enum HelloError {
    #[error("协议版本不匹配：daemon {daemon}，worker {worker}")]
    Protocol { daemon: u32, worker: u32 },
    #[error("backend 不是 qwen3：{0}")]
    Backend(String),
    #[error("model_dir 不存在或不是绝对路径：{0}")]
    ModelDir(String),
    #[error("不支持的设备：{0}")]
    Device(String),
}

impl HelloError {
    pub fn code(&self) -> ErrorCode {
        match self {
            Self::Protocol { .. } | Self::Backend(_) => ErrorCode::ProtocolMismatch,
            Self::ModelDir(_) => ErrorCode::LoadFailed,
            Self::Device(_) => ErrorCode::DeviceUnavailable,
        }
    }
}

pub fn validate_hello(
    protocol: u32,
    backend: GpuBackend,
    model_dir: &str,
    device: &str,
) -> Result<(), HelloError> {
    if protocol != GPU_PROTOCOL_VERSION {
        return Err(HelloError::Protocol {
            daemon: protocol,
            worker: GPU_PROTOCOL_VERSION,
        });
    }
    if backend != GpuBackend::Qwen3 {
        return Err(HelloError::Backend(backend.to_string()));
    }
    let path = std::path::Path::new(model_dir);
    if !path.is_absolute() || !path.is_dir() {
        return Err(HelloError::ModelDir(model_dir.to_string()));
    }
    if !device.is_empty() && !device.starts_with("vulkan") && !device.starts_with("cuda") {
        return Err(HelloError::Device(device.to_string()));
    }
    Ok(())
}

pub fn run(engine: &mut Engine, manifest: &Manifest) -> anyhow::Result<RunExit> {
    let stdin = std::io::stdin();
    let (tx, rx) = crossbeam_channel::unbounded();
    let cancel = CancelFlag::new();
    let reader_cancel = cancel.clone();
    std::thread::Builder::new()
        .name("voime-qwen-read".to_string())
        .spawn(move || read_loop(stdin, tx, reader_cancel))
        .context("创建 Qwen 读帧线程失败")?;
    let stdout = std::io::stdout();
    let mut writer = stdout.lock();
    let exit = loop {
        let Ok(command) = rx.recv() else {
            break RunExit::Abort;
        };
        match command {
            WorkerCommand::Transcribe {
                id,
                language,
                audio,
            } => {
                let response = handle(engine, manifest, id, &language, &audio, &cancel);
                if !cancel.is_shutdown() {
                    write_frame(&mut writer, &Frame::from_response(&response)?)?;
                }
                cancel.clear();
            }
            WorkerCommand::Reject(response) => {
                write_frame(&mut writer, &Frame::from_response(&response)?)?;
            }
            WorkerCommand::Shutdown => break RunExit::Shutdown,
            WorkerCommand::ProtocolError(message) => {
                write_frame(
                    &mut writer,
                    &Frame::from_response(&error_response(
                        0,
                        ErrorCode::ProtocolMismatch,
                        message,
                    ))?,
                )?;
                break RunExit::Abort;
            }
            WorkerCommand::Abort => break RunExit::Abort,
        }
    };
    Ok(exit)
}

fn read_loop(stdin: std::io::Stdin, tx: Sender<WorkerCommand>, cancel: CancelFlag) {
    let mut reader = std::io::BufReader::new(stdin);
    loop {
        let frame = match read_frame(&mut reader) {
            Ok(Some(frame)) => frame,
            Ok(None) => {
                let _ = tx.send(WorkerCommand::Abort);
                break;
            }
            Err(_) => {
                let _ = tx.send(WorkerCommand::Abort);
                break;
            }
        };
        match frame.request() {
            Ok(Request::Transcribe {
                id,
                sample_rate,
                language,
            }) => {
                if sample_rate != SAMPLE_RATE {
                    let _ = tx.send(WorkerCommand::Reject(error_response(
                        id,
                        ErrorCode::InferenceFailed,
                        format!("sample_rate 必须为 16000，实际 {sample_rate}"),
                    )));
                    continue;
                }
                if cancel.in_flight() {
                    let _ = tx.send(WorkerCommand::ProtocolError(
                        "上一段 Transcribe 尚未结束".to_string(),
                    ));
                    break;
                }
                cancel.arm(id);
                if tx
                    .send(WorkerCommand::Transcribe {
                        id,
                        language,
                        audio: frame.audio,
                    })
                    .is_err()
                {
                    break;
                }
            }
            Ok(Request::Cancel { id }) => cancel.cancel(id),
            Ok(Request::Shutdown) => {
                cancel.request_shutdown();
                let _ = tx.send(WorkerCommand::Shutdown);
                break;
            }
            Ok(Request::Hello { .. }) => {
                let _ = tx.send(WorkerCommand::ProtocolError("重复 Hello".to_string()));
                break;
            }
            Err(_) => {
                let _ = tx.send(WorkerCommand::Abort);
                break;
            }
        }
    }
}

fn handle(
    engine: &mut Engine,
    manifest: &Manifest,
    id: u64,
    language: &str,
    audio: &[f32],
    cancel: &CancelFlag,
) -> Response {
    if audio.len() > manifest.max_segment_samples() {
        return error_response(id, ErrorCode::InferenceFailed, "语音段超过 30 s");
    }
    if audio.is_empty() {
        return error_response(id, ErrorCode::InferenceFailed, "音频为空");
    }
    let started = Instant::now();
    let deadline = started + Duration::from_millis(TRANSCRIBE_BUDGET_MS);
    match engine.transcribe(audio, Some(language), cancel, deadline) {
        Ok(text) => {
            let elapsed = started.elapsed().as_millis() as u64;
            let seconds = audio.len() as f32 / SAMPLE_RATE as f32;
            Response::Result {
                id,
                text,
                latency_ms: elapsed,
                rtf: if seconds > 0.0 {
                    elapsed as f32 / 1000.0 / seconds
                } else {
                    0.0
                },
            }
        }
        Err(error) if format!("{error:#}").contains("cancelled") => {
            error_response(id, ErrorCode::InferenceFailed, "cancelled")
        }
        Err(error) if format!("{error:#}").contains("timeout") => {
            error_response(id, ErrorCode::Timeout, format!("{error:#}"))
        }
        Err(error) => error_response(id, ErrorCode::InferenceFailed, format!("{error:#}")),
    }
}

#[allow(dead_code)]
fn budget() -> Duration {
    Duration::from_millis(TRANSCRIBE_BUDGET_MS)
}
