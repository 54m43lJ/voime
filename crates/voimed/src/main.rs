use std::process::ExitCode;

use anyhow::Context;
use voime_core::audio::capture::{self, CaptureHandle, AUDIO_CHANNEL_CAPACITY};
use voime_core::config::{self, Args};
use voime_core::infer::session::SessionFactory;
use voime_core::pipeline;

use voimed::cli;
use voimed::daemon::Daemon;
use voimed::ipc_server::{self, IpcServer};

fn main() -> ExitCode {
    let cli = cli::parse();
    let socket = cli.socket.clone().unwrap_or_else(config::default_socket);

    if ipc_server::daemon_running(&socket) {
        println!("voimed: daemon 已在运行（{}）", socket.display());
        return ExitCode::SUCCESS;
    }

    let args = Args {
        model_dir: cli.model_dir,
        input_device: cli.input_device,
        threads: cli.threads,
        socket: Some(socket),
        verbose: cli.verbose,
    };
    match run(args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("voimed: {error:#}");
            ExitCode::from(1)
        }
    }
}

fn run(args: Args) -> anyhow::Result<()> {
    let verbose = args.verbose;
    let config = config::resolve(args).context("初始化配置失败")?;
    if verbose {
        eprintln!(
            "voimed: 模型 {}，socket {}，线程 {}",
            config.model_dir.display(),
            config.socket.display(),
            config.threads
        );
    }

    let factory = SessionFactory::new(config.threads).context("初始化 ONNX Runtime 失败")?;
    let (audio_tx, audio_rx) = crossbeam_channel::bounded(AUDIO_CHANNEL_CAPACITY);
    let pipeline = pipeline::spawn(&config, factory, audio_rx).context("加载语音模型失败")?;

    let audio = match capture::start(config.input_device.as_deref(), audio_tx) {
        Ok(audio) => audio,
        Err(error) => {
            let _ = pipeline.shutdown();
            return Err(error).context("打开录音设备失败");
        }
    };
    let ipc = match IpcServer::bind(&config.socket) {
        Ok(ipc) => ipc,
        Err(error) => {
            stop_audio(audio);
            let _ = pipeline.shutdown();
            return Err(error).context("启动 IPC 服务失败");
        }
    };

    Daemon::new(config, pipeline, audio, ipc).run()
}

fn stop_audio(audio: CaptureHandle) {
    audio.stop();
}
