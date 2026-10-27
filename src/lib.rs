pub mod app;
pub mod asr;
pub mod audio;
pub mod cli;
pub mod clipboard;
pub mod config;
pub mod infer;
pub mod model;
pub mod pipeline;
pub mod punc;
pub mod ui;
pub mod vad;

use std::io::IsTerminal;

use anyhow::Context;

/// Process entry shared by the binary and integration tests.
pub fn run(args: cli::Args) -> anyhow::Result<()> {
    if !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal() {
        anyhow::bail!("终端不可用：stdin 与 stdout 必须是 TTY");
    }

    let config = config::Config::resolve(args).context("解析配置失败")?;
    let factory =
        infer::session::SessionFactory::new(config.threads).context("初始化 ONNX Runtime 失败")?;

    let (audio_tx, audio_rx) = crossbeam_channel::bounded(audio::capture::AUDIO_CHANNEL_CAPACITY);
    let mut pipeline = pipeline::spawn(&config, factory, audio_rx).context("加载语音模型失败")?;

    let capture = match audio::capture::start(config.input_device.as_deref(), audio_tx) {
        Ok(capture) => capture,
        Err(error) => {
            let _ = pipeline.shutdown();
            return Err(error).context("打开录音设备失败");
        }
    };

    let clipboard = match clipboard::ClipboardWriter::new() {
        Ok(clipboard) => clipboard,
        Err(error) => {
            capture.stop();
            let _ = pipeline.shutdown();
            return Err(error).context("初始化剪贴板失败");
        }
    };

    let result = app::run(config.mode, &mut pipeline, clipboard);
    let shutdown = pipeline.shutdown();
    capture.stop();
    result?;
    shutdown
}
