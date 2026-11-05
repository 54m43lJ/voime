use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Instant;

use anyhow::Context;
use crossbeam_channel::{Receiver, Sender};
use voime_gpu_common::{
    error_response, read_frame, write_frame, ErrorCode, Frame, GpuBackend, Request, Response,
    GPU_PROTOCOL_VERSION,
};

use crate::asr::Aed;
use crate::error::WorkerError;
use crate::model::{self, ModelSet};
use crate::punc::Punc;
use crate::runtime::Runtime;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RunExit {
    Shutdown,
    Abort,
}

#[derive(Clone, Debug)]
pub enum WorkerCommand {
    Transcribe {
        id: u64,
        sample_rate: u32,
        audio: Vec<f32>,
    },
    Shutdown,
    ProtocolError(String),
    Abort,
}

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

    pub fn in_flight(&self) -> bool {
        self.in_flight.load(Ordering::Relaxed)
    }

    pub fn request_shutdown(&self) {
        self.shutdown.store(true, Ordering::Relaxed);
        self.flag.store(true, Ordering::Relaxed);
    }

    pub fn is_shutdown(&self) -> bool {
        self.shutdown.load(Ordering::Relaxed)
    }

    pub fn cancel(&self, id: u64) {
        if self.current.load(Ordering::Relaxed) == id {
            self.flag.store(true, Ordering::Relaxed);
        }
    }

    pub fn is_cancelled(&self) -> bool {
        self.flag.load(Ordering::Relaxed)
    }

    pub fn flag(&self) -> &AtomicBool {
        &self.flag
    }

    pub fn clear(&self) {
        self.current.store(0, Ordering::Relaxed);
        self.flag.store(false, Ordering::Relaxed);
        self.in_flight.store(false, Ordering::Relaxed);
    }
}

impl Default for CancelFlag {
    fn default() -> Self {
        Self::new()
    }
}

pub struct Worker {
    runtime: Runtime,
    aed: Aed,
    punc: Punc,
    cancel: CancelFlag,
    max_samples: usize,
}

pub fn validate_hello(
    protocol: u32,
    backend: GpuBackend,
    model_dir: &str,
    device: &str,
) -> anyhow::Result<()> {
    if protocol != GPU_PROTOCOL_VERSION {
        return Err(WorkerError::InferenceFailed(format!(
            "协议版本不匹配：daemon {protocol}，worker {GPU_PROTOCOL_VERSION}"
        ))
        .into());
    }
    if backend != GpuBackend::Firered {
        return Err(WorkerError::InferenceFailed("backend 不是 firered".to_string()).into());
    }
    let path = Path::new(model_dir);
    if !path.is_absolute() || !path.is_dir() {
        return Err(WorkerError::LoadFailed(format!(
            "model_dir 不存在或不是绝对路径：{model_dir}"
        ))
        .into());
    }
    if !device.is_empty()
        && !device.starts_with("cuda")
        && !device.starts_with("migraphx")
        && !device.starts_with("dml")
    {
        return Err(WorkerError::DeviceUnavailable(format!("不支持的设备：{device}")).into());
    }
    Ok(())
}

pub fn read_loop(tx: Sender<WorkerCommand>, cancel: CancelFlag) {
    let stdin = std::io::stdin();
    let mut reader = stdin.lock();
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
                id, sample_rate, ..
            }) => {
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
                        sample_rate,
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

impl Worker {
    pub fn new(models: ModelSet, runtime: Runtime) -> anyhow::Result<Self> {
        let aed = Aed::new(&models.asr, &runtime)?;
        let punc = Punc::new(&models.punc, &runtime)?;
        let max_samples = (models.asr.runtime.max_segment_ms as usize * 16_000) / 1000;
        Ok(Self {
            runtime,
            aed,
            punc,
            cancel: CancelFlag::new(),
            max_samples,
        })
    }

    pub fn cancel_flag(&self) -> CancelFlag {
        self.cancel.clone()
    }

    pub fn run(&mut self, commands: Receiver<WorkerCommand>) -> anyhow::Result<RunExit> {
        let stdout = std::io::stdout();
        let mut writer = stdout.lock();
        let exit = loop {
            let Ok(command) = commands.recv() else {
                break RunExit::Abort;
            };
            match command {
                WorkerCommand::Transcribe {
                    id,
                    sample_rate,
                    audio,
                } => {
                    let response = self.handle(id, sample_rate, &audio);
                    if !self.cancel.is_shutdown() {
                        write_frame(&mut writer, &Frame::from_response(&response)?)?;
                    }
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

    fn handle(&mut self, id: u64, sample_rate: u32, audio: &[f32]) -> Response {
        if sample_rate != 16_000 {
            self.cancel.clear();
            return error_response(
                id,
                ErrorCode::InferenceFailed,
                format!("sample_rate 必须为 16000，实际 {sample_rate}"),
            );
        }
        if audio.is_empty() {
            self.cancel.clear();
            return error_response(id, ErrorCode::InferenceFailed, "音频为空");
        }
        if audio.len() > self.max_samples {
            self.cancel.clear();
            return error_response(
                id,
                ErrorCode::InferenceFailed,
                format!("语音段超过 30 s：{} 样本", audio.len()),
            );
        }
        let started = Instant::now();
        let text = self
            .aed
            .transcribe(audio, self.cancel.flag())
            .and_then(|text| {
                if text.trim().is_empty() {
                    Ok(text)
                } else {
                    self.punc.punctuate(&text, self.cancel.flag())
                }
            });
        self.cancel.clear();
        match text {
            Ok(text) => {
                let elapsed = started.elapsed().as_millis() as u64;
                let seconds = audio.len() as f32 / 16_000.0;
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
            Err(error) => error_response(id, ErrorCode::InferenceFailed, format!("{error:#}")),
        }
    }

    pub fn ep_name(&self) -> &'static str {
        self.runtime.ep_name()
    }

    pub fn device(&self) -> &str {
        self.runtime.device()
    }

    pub fn vram_mb(&self) -> u32 {
        std::env::var("VOIME_GPU_VRAM_MB")
            .ok()
            .and_then(|value| value.parse().ok())
            .unwrap_or(0)
    }

    pub fn load_models(root: &Path) -> anyhow::Result<ModelSet> {
        model::load(root).context("加载 FireRed 模型失败")
    }
}
