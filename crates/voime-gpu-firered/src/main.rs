use std::path::Path;
use std::process::ExitCode;

use anyhow::Context;
use voime_gpu_common::{
    error_response, read_frame, write_frame, ErrorCode, Frame, GpuBackend, Request, Response,
    GPU_PROTOCOL_VERSION,
};

use voime_gpu_firered::config;
use voime_gpu_firered::error::WorkerError;
use voime_gpu_firered::model;
use voime_gpu_firered::runtime::Runtime;
use voime_gpu_firered::worker::{self, RunExit, Worker};

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("voime-gpu-firered: {error:#}");
            ExitCode::from(1)
        }
    }
}

fn run() -> anyhow::Result<()> {
    let args = config::parse()?;
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
        let code = match error.downcast_ref::<WorkerError>() {
            Some(WorkerError::DeviceUnavailable(_)) => ErrorCode::DeviceUnavailable,
            Some(WorkerError::LoadFailed(_)) => ErrorCode::LoadFailed,
            Some(WorkerError::RuntimeMissing(_)) => ErrorCode::RuntimeMissing,
            _ => ErrorCode::ProtocolMismatch,
        };
        send_error(0, code, format!("{error:#}"))?;
        return Err(error);
    }

    let models = match model::load(Path::new(&model_dir)) {
        Ok(models) => models,
        Err(error) => {
            send_error(0, ErrorCode::LoadFailed, format!("{error:#}"))?;
            return Err(error);
        }
    };
    let runtime = match Runtime::load(args.runtime.expect("runtime defaulted"), &device) {
        Ok(runtime) => runtime,
        Err(error) => {
            let code = match &error.downcast_ref::<WorkerError>() {
                Some(WorkerError::RuntimeMissing(_)) => ErrorCode::RuntimeMissing,
                Some(WorkerError::DeviceUnavailable(_)) => ErrorCode::DeviceUnavailable,
                _ => ErrorCode::RuntimeMissing,
            };
            send_error(0, code, format!("{error:#}"))?;
            return Err(error);
        }
    };
    let mut worker = match Worker::new(models, runtime) {
        Ok(worker) => worker,
        Err(error) => {
            send_error(0, ErrorCode::LoadFailed, format!("{error:#}"))?;
            return Err(error);
        }
    };

    let ready = Response::Ready {
        protocol: GPU_PROTOCOL_VERSION,
        backend: GpuBackend::Firered,
        runtime: worker.ep_name().to_string(),
        device: worker.device().to_string(),
        vram_mb: worker.vram_mb(),
        punc: true,
    };
    {
        let stdout = std::io::stdout();
        let mut writer = stdout.lock();
        write_frame(&mut writer, &Frame::from_response(&ready)?)?;
    }

    let (tx, rx) = crossbeam_channel::unbounded();
    let cancel = worker.cancel_flag();
    std::thread::Builder::new()
        .name("voime-firered-read".to_string())
        .spawn(move || worker::read_loop(tx, cancel))
        .context("创建读帧线程失败")?;
    let exit = worker.run(rx)?;
    drop(worker);
    match exit {
        RunExit::Shutdown => {
            let stdout = std::io::stdout();
            let mut writer = stdout.lock();
            write_frame(&mut writer, &Frame::from_response(&Response::Bye)?)?;
            Ok(())
        }
        RunExit::Abort => anyhow::bail!("daemon 在未发送 Shutdown 的情况下关闭了 worker 连接"),
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
