use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::time::Duration;

use anyhow::{bail, Context};
use crossbeam_channel::{Receiver, RecvTimeoutError};
use serde::{Deserialize, Serialize};
use voime_core::asr::r#final::{AsrOutput, FinalBackend};
use voime_log::vlog;

/// Worker protocol is independent of the frontend IPC version.
pub const WORKER_PROTOCOL_VERSION: u32 = 1;
const MAX_HEADER_BYTES: u32 = 1024 * 1024;
const MAX_AUDIO_BYTES: u32 = 64 * 1024 * 1024;
const LOAD_TIMEOUT: Duration = Duration::from_secs(120);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(60);

#[derive(Debug, Serialize)]
enum Request {
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

#[derive(Clone, Debug, Deserialize)]
pub struct Ready {
    pub protocol: u32,
    pub backend: String,
    pub provider: String,
    pub device: String,
    pub vram_mb: u32,
    pub supports_partial: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
enum Response {
    Ready {
        protocol: u32,
        backend: String,
        provider: String,
        device: String,
        #[serde(default)]
        vram_mb: u32,
        #[serde(default)]
        supports_partial: bool,
    },
    Result {
        id: u64,
        text: String,
        #[serde(default)]
        latency_ms: u64,
        #[serde(default)]
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

pub struct WorkerBackend {
    child: Child,
    stdin: ChildStdin,
    responses: Receiver<anyhow::Result<Response>>,
    next_id: u64,
    manifest_dir: PathBuf,
    device: String,
    language: Option<String>,
    use_itn: bool,
    load_timeout: Duration,
    request_timeout: Duration,
}

impl WorkerBackend {
    /// Spawns `voime-gpu` for a Final package directory.
    pub fn spawn(manifest: &Path, device: &str) -> anyhow::Result<Self> {
        let binary = worker_binary()
            .context("找不到 voime-gpu；请安装 GPU Final 二进制或设置 VOIME_GPU_BIN")?;
        let mut command = Command::new(&binary);
        command
            .arg("--manifest")
            .arg(manifest)
            .arg("--device")
            .arg(device)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if let Some(dir) = runtime_directory() {
            command.env("ORT_DYLIB_PATH", dir);
        }
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            command.process_group(0);
        }
        let mut child = command
            .spawn()
            .with_context(|| format!("启动 voime-gpu 失败 {}", binary.display()))?;
        let stdin = child.stdin.take().context("voime-gpu stdin 不可用")?;
        let stdout = child.stdout.take().context("voime-gpu stdout 不可用")?;
        let stderr = child.stderr.take().context("voime-gpu stderr 不可用")?;

        let (tx, responses) = crossbeam_channel::bounded(4);
        std::thread::Builder::new()
            .name("voime-gpu-reader".to_string())
            .spawn(move || {
                let mut reader = BufReader::new(stdout);
                loop {
                    match read_response_frame(&mut reader) {
                        Ok(Some(response)) => {
                            if tx.send(Ok(response)).is_err() {
                                break;
                            }
                        }
                        Ok(None) => break,
                        Err(error) => {
                            let _ = tx.send(Err(error));
                            break;
                        }
                    }
                }
            })
            .context("创建 worker reader 线程失败")?;
        std::thread::Builder::new()
            .name("voime-gpu-stderr".to_string())
            .spawn(move || {
                let mut reader = BufReader::new(stderr);
                let mut line = String::new();
                while reader.read_line(&mut line).unwrap_or(0) > 0 {
                    vlog!("voime-gpu: {}", line.trim_end());
                    line.clear();
                }
            })
            .ok();

        Ok(Self {
            child,
            stdin,
            responses,
            next_id: 1,
            manifest_dir: manifest.to_path_buf(),
            device: device.to_string(),
            language: None,
            use_itn: false,
            load_timeout: LOAD_TIMEOUT,
            request_timeout: REQUEST_TIMEOUT,
        })
    }

    pub fn handshake(&mut self) -> anyhow::Result<Ready> {
        self.send(&Request::Hello {
            protocol: WORKER_PROTOCOL_VERSION,
            manifest: self.manifest_dir.to_string_lossy().to_string(),
            device: self.device.clone(),
        })?;
        match self.recv(self.load_timeout)? {
            Response::Ready {
                protocol,
                backend,
                provider,
                device,
                vram_mb,
                supports_partial,
            } => {
                if protocol != WORKER_PROTOCOL_VERSION {
                    bail!(
                        "worker 协议版本 {} 与 daemon {} 不一致",
                        protocol,
                        WORKER_PROTOCOL_VERSION
                    );
                }
                Ok(Ready {
                    protocol,
                    backend,
                    provider,
                    device,
                    vram_mb,
                    supports_partial,
                })
            }
            Response::Error { code, message, .. } => {
                bail!("worker 加载失败（{code}）：{message}")
            }
            other => bail!("worker 握手响应非法：{other:?}"),
        }
    }

    pub fn transcribe(&mut self, samples: &[f32]) -> anyhow::Result<String> {
        let id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1).max(1);
        write_request_frame(
            &mut self.stdin,
            &Request::Transcribe {
                id,
                sample_rate: 16_000,
                language: self.language.clone(),
                use_itn: self.use_itn,
            },
            samples,
        )?;
        let response = match self.responses.recv_timeout(self.request_timeout) {
            Ok(Ok(response)) => response,
            Ok(Err(error)) => return Err(error),
            Err(RecvTimeoutError::Timeout) => {
                let _ = self.send(&Request::Cancel { id });
                self.kill();
                bail!("等待 worker 响应超时");
            }
            Err(RecvTimeoutError::Disconnected) => bail!("worker 连接已断开"),
        };
        match response {
            Response::Result {
                id: result_id,
                text,
                latency_ms,
                rtf,
            } => {
                if result_id != id {
                    bail!("worker 结果 id {result_id} 与请求 {id} 不一致");
                }
                vlog!(
                    "worker Final chars={} latency={}ms rtf={:.3}",
                    text.chars().count(),
                    latency_ms,
                    rtf
                );
                Ok(text)
            }
            Response::Error {
                code,
                message,
                id: error_id,
            } => {
                if let Some(error_id) = error_id {
                    if error_id != id {
                        bail!("worker 错误 id {error_id} 与请求 {id} 不一致");
                    }
                }
                if code == "Timeout" {
                    self.kill();
                }
                bail!("worker 推理失败（{code}）：{message}")
            }
            other => bail!("worker 响应非法：{other:?}"),
        }
    }

    pub fn restart(&mut self) -> anyhow::Result<()> {
        let _ = self.shutdown();
        let mut replacement = Self::spawn(&self.manifest_dir, &self.device)?;
        replacement.handshake()?;
        *self = replacement;
        Ok(())
    }

    pub fn shutdown(&mut self) -> anyhow::Result<()> {
        if self.send(&Request::Shutdown).is_err() {
            self.kill();
            return Ok(());
        }
        match self.recv(Duration::from_secs(5)) {
            Ok(Response::Bye) | Err(_) => {
                self.kill();
                Ok(())
            }
            Ok(_) => {
                self.kill();
                Ok(())
            }
        }
    }

    fn send(&mut self, request: &Request) -> anyhow::Result<()> {
        write_request_frame(&mut self.stdin, request, &[])
    }

    fn recv(&self, timeout: Duration) -> anyhow::Result<Response> {
        match self.responses.recv_timeout(timeout) {
            Ok(Ok(response)) => Ok(response),
            Ok(Err(error)) => Err(error),
            Err(RecvTimeoutError::Timeout) => bail!("等待 worker 响应超时"),
            Err(RecvTimeoutError::Disconnected) => bail!("worker 连接已断开"),
        }
    }

    fn kill(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl FinalBackend for WorkerBackend {
    fn transcribe(&mut self, samples: &[f32]) -> anyhow::Result<AsrOutput> {
        let text = WorkerBackend::transcribe(self, samples)?;
        Ok(AsrOutput {
            ids: Vec::new(),
            text,
        })
    }
}

impl Drop for WorkerBackend {
    fn drop(&mut self) {
        self.kill();
    }
}

pub fn worker_binary() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("VOIME_GPU_BIN") {
        let path = PathBuf::from(path);
        if path.is_file() {
            return Some(path);
        }
    }
    let exe = std::env::current_exe().ok()?;
    let name = if cfg!(windows) {
        "voime-gpu.exe"
    } else {
        "voime-gpu"
    };
    let sibling = exe.parent()?.join(name);
    sibling.is_file().then_some(sibling)
}

fn runtime_directory() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("VOIME_GPU_RUNTIME_DIR") {
        let dir = PathBuf::from(dir);
        if dir.is_dir() {
            return Some(dir);
        }
    }
    let base = PathBuf::from(std::env::var_os("VOIME_RUNTIME_DIR")?);
    let entries = std::fs::read_dir(&base).ok()?;
    for entry in entries.flatten() {
        let dir = entry.path();
        if dir.is_dir() {
            return Some(dir);
        }
    }
    None
}

fn write_request_frame(
    writer: &mut impl Write,
    request: &Request,
    samples: &[f32],
) -> anyhow::Result<()> {
    let header = serde_json::to_vec(request).context("序列化 worker 请求失败")?;
    if header.len() > MAX_HEADER_BYTES as usize {
        bail!("worker 请求头超过 {MAX_HEADER_BYTES} 字节");
    }
    let audio_bytes = samples.len().saturating_mul(4);
    if audio_bytes > MAX_AUDIO_BYTES as usize {
        bail!("worker 单帧音频超过 {MAX_AUDIO_BYTES} 字节");
    }
    writer
        .write_all(&(header.len() as u32).to_le_bytes())
        .context("写入 worker 头长度失败")?;
    writer
        .write_all(&header)
        .context("写入 worker 请求头失败")?;
    writer
        .write_all(&(audio_bytes as u32).to_le_bytes())
        .context("写入 worker 音频长度失败")?;
    for sample in samples {
        writer
            .write_all(&sample.to_le_bytes())
            .context("写入 worker 音频失败")?;
    }
    writer.flush().context("刷新 worker 请求失败")?;
    Ok(())
}

fn read_response_frame(reader: &mut impl Read) -> anyhow::Result<Option<Response>> {
    let Some(header_len) = read_u32(reader)? else {
        return Ok(None);
    };
    if header_len == 0 || header_len > MAX_HEADER_BYTES {
        bail!("worker 响应头长度非法：{header_len}");
    }
    let mut header = vec![0u8; header_len as usize];
    reader
        .read_exact(&mut header)
        .context("读取 worker 响应头失败")?;
    let response: Response =
        serde_json::from_slice(&header).context("worker 响应头不是合法 JSON")?;
    let Some(audio_len) = read_u32(reader)? else {
        bail!("worker 响应缺少音频长度");
    };
    if audio_len > MAX_AUDIO_BYTES {
        bail!("worker 响应音频长度非法：{audio_len}");
    }
    if audio_len > 0 {
        let mut discard = vec![0u8; audio_len as usize];
        reader
            .read_exact(&mut discard)
            .context("读取 worker 响应音频失败")?;
    }
    Ok(Some(response))
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
    fn response_frame_round_trip() {
        let header = serde_json::to_vec(&Response::Result {
            id: 7,
            text: "你好".to_string(),
            latency_ms: 12,
            rtf: 0.1,
        })
        .unwrap();
        let mut bytes = Vec::new();
        bytes.extend((header.len() as u32).to_le_bytes());
        bytes.extend(&header);
        bytes.extend(0u32.to_le_bytes());
        let mut cursor = std::io::Cursor::new(bytes);
        let response = read_response_frame(&mut cursor).unwrap().unwrap();
        match response {
            Response::Result { id, text, .. } => {
                assert_eq!(id, 7);
                assert_eq!(text, "你好");
            }
            other => panic!("unexpected {other:?}"),
        }
    }
}
