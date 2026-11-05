use std::path::Path;
use std::process::ExitCode;

use anyhow::Context;
use voime_gpu_common::{
    error_response, read_frame, write_frame, ErrorCode, Frame, GpuBackend, Request, Response,
    GPU_PROTOCOL_VERSION,
};

use voime_gpu_qwen3::engine::Engine;
use voime_gpu_qwen3::manifest::Manifest;
use voime_gpu_qwen3::worker;

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("voime-gpu-qwen3: {error:#}");
            ExitCode::from(1)
        }
    }
}

fn run() -> anyhow::Result<()> {
    let stdin = std::io::stdin();
    let frame = {
        let mut reader = stdin.lock();
        read_frame(&mut reader)?.context("worker 在 Hello 前收到 EOF")?
    };
    let hello = frame.request()?;
    let Request::Hello {
        protocol,
        backend,
        model_dir,
        device,
    } = hello
    else {
        send_error(
            0,
            ErrorCode::ProtocolMismatch,
            "首条请求必须是 Hello".to_string(),
        )?;
        anyhow::bail!("首条请求不是 Hello");
    };
    if let Err(error) = worker::validate_hello(protocol, backend, &model_dir, &device) {
        send_error(0, error.code(), error.to_string())?;
        return Err(error.into());
    }

    let runtime = match (cfg!(feature = "vulkan"), cfg!(feature = "cuda")) {
        (true, false) => "vulkan",
        (false, true) => "cuda",
        (true, true) => {
            send_error(
                0,
                ErrorCode::ProtocolMismatch,
                "worker 同时启用了 vulkan 与 cuda".to_string(),
            )?;
            anyhow::bail!("Qwen worker 只允许一个 backend feature");
        }
        (false, false) => {
            send_error(
                0,
                ErrorCode::RuntimeMissing,
                "worker 未启用 vulkan / cuda backend".to_string(),
            )?;
            anyhow::bail!("Qwen worker 未启用 backend feature");
        }
    };
    if !device.is_empty() && !device.starts_with(runtime) {
        send_error(
            0,
            ErrorCode::DeviceUnavailable,
            format!("请求设备 {device} 与编译时 backend {runtime} 不匹配"),
        )?;
        anyhow::bail!("Qwen 设备与 backend 不匹配");
    }
    eprintln!(
        "voime-gpu-qwen3: llama.cpp {}",
        llama_cpp_4::llama_version()
    );

    let dir = Path::new(&model_dir);
    let manifest = match Manifest::load(dir) {
        Ok(manifest) => manifest,
        Err(error) => {
            send_error(0, ErrorCode::LoadFailed, format!("{error:#}"))?;
            return Err(error);
        }
    };
    if let Err(error) = manifest.verify(dir) {
        send_error(0, ErrorCode::LoadFailed, format!("{error:#}"))?;
        return Err(error);
    }
    let mut engine = match Engine::load(&manifest, dir, &device) {
        Ok(engine) => engine,
        Err(error) => {
            let text = format!("{error:#}");
            let code = if text.contains("backend") || text.contains("runtime") {
                ErrorCode::RuntimeMissing
            } else if text.contains("设备")
                || text.contains("device")
                || text.contains("GPU")
                || text.contains("Vulkan")
                || text.contains("CUDA")
            {
                ErrorCode::DeviceUnavailable
            } else {
                ErrorCode::LoadFailed
            };
            send_error(0, code, text)?;
            return Err(error);
        }
    };

    let ready = Response::Ready {
        protocol: GPU_PROTOCOL_VERSION,
        backend: GpuBackend::Qwen3,
        runtime: runtime.to_string(),
        device: if device.is_empty() {
            format!("{runtime}:0")
        } else {
            device.clone()
        },
        vram_mb: std::env::var("VOIME_GPU_VRAM_MB")
            .ok()
            .and_then(|value| value.parse().ok())
            .unwrap_or(0),
        punc: false,
    };
    {
        let stdout = std::io::stdout();
        let mut writer = stdout.lock();
        write_frame(&mut writer, &Frame::from_response(&ready)?)?;
    }
    let exit = worker::run(&mut engine, &manifest)?;
    drop(engine);
    match exit {
        worker::RunExit::Shutdown => {
            let stdout = std::io::stdout();
            let mut writer = stdout.lock();
            write_frame(&mut writer, &Frame::from_response(&Response::Bye)?)?;
            Ok(())
        }
        worker::RunExit::Abort => {
            anyhow::bail!("daemon 在未发送 Shutdown 的情况下关闭了 worker 连接")
        }
    }
}

fn send_error(id: u64, code: ErrorCode, message: String) -> anyhow::Result<()> {
    let stdout = std::io::stdout();
    let mut writer = stdout.lock();
    write_frame(
        &mut writer,
        &Frame::from_response(&error_response(id, code, message))?,
    )?;
    Ok(())
}
