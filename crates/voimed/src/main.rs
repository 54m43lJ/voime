use std::process::ExitCode;

use anyhow::Context;
use voime_core::audio::capture::{self, CaptureHandle, AUDIO_CHANNEL_CAPACITY};
use voime_core::infer::session::SessionFactory;
use voime_core::pipeline;
use voime_log::vlog;
use voime_transport::{default_endpoint, Endpoint};

use voimed::backend::BackendId;
use voimed::cli;
use voimed::config;
use voimed::daemon::Daemon;
use voimed::ipc_server::{self, IpcServer};
use voimed::worker::Worker;

fn main() -> ExitCode {
    voime_log::init("voimed");
    let cli = cli::parse();
    match run(cli) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("voimed: {error:#}");
            ExitCode::from(1)
        }
    }
}

fn run(cli: cli::Cli) -> anyhow::Result<()> {
    if cli.verbose {
        eprintln!("voimed: starting");
    }
    let config = config::from_cli(&cli)?;
    let endpoint = match config.endpoint.as_deref() {
        Some(value) => Endpoint::parse(value).context("解析端点失败")?,
        None => default_endpoint(),
    };
    if ipc_server::daemon_running(&endpoint) {
        vlog!("daemon already running at {endpoint}");
        return Ok(());
    }

    if cli.verbose {
        eprintln!(
            "voimed: model_dir={} backend={} endpoint={}",
            config.model_dir.display(),
            config.final_config.backend,
            endpoint
        );
    }
    let core_config = voime_core::config::resolve(voime_core::config::Args {
        model_root: Some(config.model_dir.clone()),
        input_device: config.input_device.clone(),
        threads: Some(config.threads),
        verbose: cli.verbose,
    })?;
    let factory = SessionFactory::new(config.threads)?;
    let (audio_tx, audio_rx) = crossbeam_channel::bounded(AUDIO_CHANNEL_CAPACITY);
    let (pipeline, finalizer) = pipeline::spawn(&core_config, factory, audio_rx)?;

    let mut worker = None;
    let mut worker_error = None;
    if config.final_config.backend != BackendId::Cpu {
        match start_worker(&config.model_dir, config.final_config.backend, cli.verbose) {
            Ok(started) => {
                worker = Some(started);
                vlog!("gpu worker {} ready", config.final_config.backend);
            }
            Err(error) => {
                // GPU failure never blocks daemon startup; CPU Final stays available.
                vlog!(
                    "gpu worker {} unavailable: {error:#}",
                    config.final_config.backend
                );
                worker_error = Some(format!("{error:#}"));
            }
        }
    }

    let audio =
        capture::start(config.input_device.as_deref(), audio_tx).context("启动音频采集失败")?;
    let ipc = IpcServer::bind(&endpoint)?;
    vlog!("voimed listening on {endpoint}");
    if cli.verbose {
        eprintln!("voimed: listening on {endpoint}");
    }
    Daemon::new(
        config,
        pipeline,
        finalizer,
        audio,
        ipc,
        worker,
        worker_error,
    )
    .run()
}

fn start_worker(
    model_root: &std::path::Path,
    backend: BackendId,
    verbose: bool,
) -> anyhow::Result<Worker> {
    for directory in backend.model_dirs() {
        if !model_root.join(directory).is_dir() {
            anyhow::bail!("缺少 GPU 模型目录 {}", model_root.join(directory).display());
        }
    }
    let log_level = if verbose { "debug" } else { "info" };
    let mut worker = Worker::spawn(backend, model_root, log_level)?;
    match worker.handshake(None) {
        Ok(info) => {
            vlog!(
                "gpu {} runtime={} device={} vram_mb={} punc={}",
                backend,
                info.runtime,
                info.device,
                info.vram_mb,
                info.punc
            );
            Ok(worker)
        }
        Err(error) => {
            worker.shutdown();
            Err(error)
        }
    }
}

#[allow(dead_code)]
fn stop_audio(audio: CaptureHandle) {
    audio.stop();
}
