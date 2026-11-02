use std::process::ExitCode;

use anyhow::Context;
use voime_core::audio::capture::{self, CaptureHandle, AUDIO_CHANNEL_CAPACITY};
use voime_core::config::{self, Args};
use voime_core::infer::session::SessionFactory;
use voime_transport::{default_endpoint, Endpoint};

use voime_log::vlog;
use voimed::cli;
use voimed::daemon::{self, Daemon, FinalRuntime};
use voimed::ipc_server::{self, IpcServer};

fn main() -> ExitCode {
    if voime_log::init("voimed") {
        if let Some(path) = voime_log::log_path() {
            eprintln!("voimed: 日志写入 {}", path.display());
        }
    }
    let cli = cli::parse();
    let endpoint = match cli
        .endpoint
        .as_deref()
        .map(Endpoint::parse)
        .transpose()
        .context("解析端点失败")
    {
        Ok(Some(endpoint)) => endpoint,
        Ok(None) => default_endpoint(),
        Err(error) => {
            eprintln!("voimed: {error:#}");
            return ExitCode::from(1);
        }
    };

    if ipc_server::daemon_running(&endpoint) {
        println!("voimed: daemon 已在运行（{endpoint}）");
        return ExitCode::SUCCESS;
    }

    let args = Args {
        input_device: cli.input_device,
        threads: cli.threads,
        verbose: cli.verbose,
    };
    match run(args, endpoint) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("voimed: {error:#}");
            ExitCode::from(1)
        }
    }
}

fn run(args: Args, endpoint: Endpoint) -> anyhow::Result<()> {
    let verbose = args.verbose;
    let config = config::resolve(args.clone()).context("初始化配置失败")?;
    vlog!(
        "voimed 启动 endpoint={endpoint} models={} gpu_final={} threads={}",
        config.model_root.display(),
        config.gpu_final(),
        config.threads
    );
    if verbose {
        eprintln!(
            "voimed: 模型根目录 {}，Final={}，端点 {}，线程 {}",
            config.model_root.display(),
            if config.gpu_final() {
                "FireRed AED (GPU)"
            } else {
                "SenseVoice (CPU)"
            },
            endpoint,
            config.threads
        );
    }

    let factory = SessionFactory::new(config.threads).context("初始化 ONNX Runtime 失败")?;
    let (audio_tx, audio_rx) = crossbeam_channel::bounded(AUDIO_CHANNEL_CAPACITY);
    let (config, final_runtime, worker) =
        daemon::build_runtime(&config, factory, audio_rx).context("加载语音模型失败")?;

    let audio = match capture::start(config.input_device.as_deref(), audio_tx) {
        Ok(audio) => audio,
        Err(error) => {
            shutdown_runtime(final_runtime, worker);
            return Err(error).context("打开录音设备失败");
        }
    };
    let ipc = match IpcServer::bind(&endpoint) {
        Ok(ipc) => ipc,
        Err(error) => {
            stop_audio(audio);
            shutdown_runtime(final_runtime, worker);
            return Err(error).context("启动 IPC 服务失败");
        }
    };

    Daemon::new(config, final_runtime, worker, audio, ipc).run()
}

fn stop_audio(audio: CaptureHandle) {
    audio.stop();
}

fn shutdown_runtime(
    runtime: FinalRuntime,
    worker: Option<std::sync::Arc<std::sync::Mutex<voimed::worker::WorkerBackend>>>,
) {
    let mut runtime = runtime;
    if let Some(worker) = worker {
        if let Ok(mut worker) = worker.lock() {
            let _ = worker.shutdown();
        }
    }
    if let Some(pipeline) = runtime.pipeline.take() {
        let _ = pipeline.shutdown();
    }
}
