use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::time::{Duration, Instant};

use anyhow::{bail, Context};
use crossbeam_channel::{Receiver, RecvTimeoutError};
use voime_gpu_common::{
    error_response, expect_result, read_frame, write_frame, ErrorCode, Frame, GpuBackend, Request,
    Response, HANDSHAKE_TIMEOUT as GPU_HANDSHAKE_TIMEOUT, SHUTDOWN_TIMEOUT as GPU_SHUTDOWN_TIMEOUT,
    TRANSCRIBE_TIMEOUT as GPU_TRANSCRIBE_TIMEOUT,
};
use voime_ipc::Mode;

use crate::backend::BackendId;

pub const LOAD_TIMEOUT: Duration = GPU_HANDSHAKE_TIMEOUT;
pub const TRANSCRIBE_TIMEOUT: Duration = GPU_TRANSCRIBE_TIMEOUT;
pub const SHUTDOWN_TIMEOUT: Duration = GPU_SHUTDOWN_TIMEOUT;
pub const MAX_RESTARTS: u32 = 3;

type ResponseReceiver = Receiver<anyhow::Result<Option<Frame>>>;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WorkerStatus {
    Starting,
    Ready,
    Running,
    Unavailable,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReadyInfo {
    pub runtime: String,
    pub device: String,
    pub vram_mb: u32,
    pub punc: bool,
}

impl Drop for Worker {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

pub struct Worker {
    id: BackendId,
    child: Child,
    stdin: ChildStdin,
    responses: ResponseReceiver,
    status: WorkerStatus,
    next_id: u64,
    model_root: PathBuf,
    log_level: String,
    runtime: Option<String>,
    device: Option<String>,
}

impl Worker {
    pub fn spawn(id: BackendId, model_root: &Path, log_level: &str) -> anyhow::Result<Self> {
        if id == BackendId::Cpu {
            bail!("CPU 后端不启动 GPU worker");
        }
        let (child, stdin, responses) = spawn_process(id, model_root, log_level)?;
        Ok(Self {
            id,
            child,
            stdin,
            responses,
            status: WorkerStatus::Starting,
            next_id: 1,
            model_root: model_root
                .canonicalize()
                .unwrap_or_else(|_| model_root.to_path_buf()),
            log_level: log_level.to_string(),
            runtime: None,
            device: None,
        })
    }

    pub fn handshake(&mut self, device: Option<&str>) -> anyhow::Result<ReadyInfo> {
        let device = device
            .map(str::to_string)
            .or_else(|| std::env::var("VOIME_GPU_DEVICE").ok())
            .unwrap_or_default();
        let model_dir = match self.id {
            BackendId::Qwen3 => self.model_root.join("qwen3-asr-1.7b"),
            BackendId::Firered => self.model_root.clone(),
            BackendId::Cpu => unreachable!("CPU backend has no worker"),
        };
        let hello = Request::Hello {
            protocol: voime_gpu_common::GPU_PROTOCOL_VERSION,
            backend: gpu_backend(self.id),
            model_dir: model_dir.display().to_string(),
            device,
        };
        let frame = Frame::from_request(&hello, &[])?;
        write_frame(&mut self.stdin, &frame).context("向 GPU worker 发送 Hello 失败")?;
        let response = self.read_response(LOAD_TIMEOUT)?;
        match response {
            Response::Ready {
                protocol,
                backend,
                runtime,
                device,
                vram_mb,
                punc,
            } => {
                if protocol != voime_gpu_common::GPU_PROTOCOL_VERSION {
                    self.status = WorkerStatus::Unavailable;
                    bail!(
                        "GPU 协议版本不匹配：daemon {}，worker {protocol}",
                        voime_gpu_common::GPU_PROTOCOL_VERSION
                    );
                }
                if backend != gpu_backend(self.id) {
                    self.status = WorkerStatus::Unavailable;
                    bail!("GPU worker 后端不匹配：期望 {}，实际 {backend}", self.id);
                }
                self.status = WorkerStatus::Ready;
                self.runtime = Some(runtime.clone());
                self.device = Some(device.clone());
                Ok(ReadyInfo {
                    runtime,
                    device,
                    vram_mb,
                    punc,
                })
            }
            Response::Error { code, message, .. } => {
                self.status = WorkerStatus::Unavailable;
                Err(map_error(code, &message))
            }
            _ => {
                self.status = WorkerStatus::Unavailable;
                bail!("GPU worker 握手响应非法")
            }
        }
    }

    pub fn transcribe(&mut self, samples: &[f32]) -> anyhow::Result<String> {
        if self.status != WorkerStatus::Ready && self.status != WorkerStatus::Running {
            bail!("GPU worker 不可用");
        }
        let id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1).max(1);
        let request = Request::Transcribe {
            id,
            sample_rate: 16_000,
            language: "auto".to_string(),
        };
        let frame = Frame::from_request(&request, samples)?;
        write_frame(&mut self.stdin, &frame).context("向 GPU worker 发送 Transcribe 失败")?;
        self.status = WorkerStatus::Running;
        voime_log::vlog!(
            "gpu worker {} running id={id} samples={}",
            self.id,
            samples.len()
        );
        let response = match self.read_response(TRANSCRIBE_TIMEOUT) {
            Ok(response) => response,
            Err(error) => {
                self.status = WorkerStatus::Unavailable;
                return Err(error);
            }
        };
        let response = expect_result(response, id)?;
        self.status = WorkerStatus::Ready;
        voime_log::vlog!("gpu worker {} ready", self.id);
        match response {
            Response::Result { text, .. } => Ok(text),
            Response::Error { code, message, .. } => Err(map_error(code, &message)),
            other => bail!("GPU worker 返回了非终态响应：{other:?}"),
        }
    }

    pub fn restart(&mut self) -> anyhow::Result<()> {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let (child, stdin, responses) = spawn_process(self.id, &self.model_root, &self.log_level)?;
        self.child = child;
        self.stdin = stdin;
        self.responses = responses;
        self.status = WorkerStatus::Starting;
        let device = self.device.clone();
        self.handshake(device.as_deref())?;
        Ok(())
    }

    pub fn shutdown(mut self) {
        if self.status != WorkerStatus::Unavailable {
            let frame = Frame::from_request(&Request::Shutdown, &[]);
            if let Ok(frame) = frame {
                let _ = write_frame(&mut self.stdin, &frame);
            }
        }
        let deadline = Instant::now() + SHUTDOWN_TIMEOUT;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                break;
            }
            match self.responses.recv_timeout(remaining) {
                Ok(Ok(Some(frame))) => {
                    if matches!(frame.response(), Ok(Response::Bye)) {
                        break;
                    }
                }
                Ok(Ok(None)) | Ok(Err(_)) | Err(RecvTimeoutError::Disconnected) => break,
                Err(RecvTimeoutError::Timeout) => break,
            }
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
        self.status = WorkerStatus::Unavailable;
    }

    pub fn status(&self) -> WorkerStatus {
        self.status
    }

    fn read_response(&mut self, timeout: Duration) -> anyhow::Result<Response> {
        let deadline = Instant::now() + timeout;
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            bail!("等待 GPU worker 响应超时");
        }
        match self.responses.recv_timeout(remaining) {
            Ok(Ok(Some(frame))) => frame.response(),
            Ok(Ok(None)) => bail!("GPU worker 已关闭连接"),
            Ok(Err(error)) => Err(error),
            Err(RecvTimeoutError::Timeout) => bail!("等待 GPU worker 响应超时"),
            Err(RecvTimeoutError::Disconnected) => bail!("GPU worker 已退出"),
        }
    }
}

fn spawn_process(
    id: BackendId,
    model_root: &Path,
    log_level: &str,
) -> anyhow::Result<(Child, ChildStdin, ResponseReceiver)> {
    let executable = worker_executable(id)?;
    let mut command = Command::new(&executable);
    if id == BackendId::Firered {
        if let Ok(runtime) = std::env::var("VOIME_FIRERED_RUNTIME") {
            command.arg("--runtime").arg(runtime);
        }
        command.arg("--log-level").arg(log_level);
    }
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000);
    }
    let mut child = command
        .spawn()
        .with_context(|| format!("启动 GPU worker 失败 {}", executable.display()))?;
    let stdin = child.stdin.take().context("GPU worker stdin 不可用")?;
    let stdout = child.stdout.take().context("GPU worker stdout 不可用")?;
    if let Some(stderr) = child.stderr.take() {
        std::thread::Builder::new()
            .name(format!("voime-worker-{}-stderr", id.as_str()))
            .spawn(move || {
                for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                    voime_log::vlog!("gpu[{}] {line}", id.as_str());
                }
            })
            .context("创建 GPU stderr 线程失败")?;
    }
    let (tx, rx) = crossbeam_channel::unbounded();
    std::thread::Builder::new()
        .name(format!("voime-worker-{}-stdout", id.as_str()))
        .spawn(move || {
            let mut reader = BufReader::new(stdout);
            loop {
                let frame = read_frame(&mut reader);
                let stop = !matches!(frame, Ok(Some(_)));
                if tx.send(frame).is_err() || stop {
                    break;
                }
            }
        })
        .context("创建 GPU stdout 线程失败")?;
    let _ = model_root;
    Ok((child, stdin, rx))
}

fn worker_executable(id: BackendId) -> anyhow::Result<PathBuf> {
    let name = id
        .worker_binary()
        .context("CPU 后端没有 worker 可执行文件")?;
    if let Some(explicit) = std::env::var_os(format!("VOIME_{}_BIN", id.as_str().to_uppercase())) {
        let path = PathBuf::from(explicit);
        if path.is_file() {
            return Ok(path);
        }
    }
    if let Ok(current) = std::env::current_exe() {
        if let Some(parent) = current.parent() {
            let sibling = parent.join(name);
            if sibling.is_file() {
                return Ok(sibling);
            }
        }
    }
    Ok(PathBuf::from(name))
}

fn gpu_backend(id: BackendId) -> GpuBackend {
    match id {
        BackendId::Firered => GpuBackend::Firered,
        BackendId::Qwen3 => GpuBackend::Qwen3,
        BackendId::Cpu => GpuBackend::Firered,
    }
}

fn map_error(code: ErrorCode, message: &str) -> anyhow::Error {
    anyhow::anyhow!("GPU worker 错误（{code}）：{message}")
}

#[allow(dead_code)]
fn cancellation_response(id: u64) -> Response {
    error_response(id, ErrorCode::InferenceFailed, "cancelled")
}

#[allow(dead_code)]
fn language_for(_mode: Mode) -> &'static str {
    "auto"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backend_mapping_has_no_cpu_worker() {
        assert!(BackendId::Cpu.worker_binary().is_none());
        assert_eq!(gpu_backend(BackendId::Firered), GpuBackend::Firered);
        assert_eq!(gpu_backend(BackendId::Qwen3), GpuBackend::Qwen3);
    }
}
