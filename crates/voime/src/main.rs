mod app;
mod cli;
mod client;
mod doctor;
mod ui;

use std::process::ExitCode;
use std::time::Duration;

use anyhow::Context;
use voime_core::config::default_model_dir;
use voime_core::model;
use voime_ipc::{BackendPhase, ClientMessage, Role, ServerMessage, TriggerSource};
use voime_transport::{default_endpoint, Endpoint};

use crate::app::App;
use crate::cli::{Cli, Command, CtlAction};
use crate::client::Client;

fn main() -> ExitCode {
    let cli = parse_cli();
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
            eprintln!("voime: {error:#}");
            return ExitCode::from(1);
        }
    };
    match cli.command {
        None => run_tui(&endpoint),
        Some(Command::Ctl(args)) => run_ctl(&endpoint, args.action),
        Some(Command::Doctor) => {
            if doctor::run(&endpoint) {
                ExitCode::SUCCESS
            } else {
                ExitCode::from(1)
            }
        }
    }
}

fn parse_cli() -> Cli {
    cli::parse()
}

fn run_tui(endpoint: &Endpoint) -> ExitCode {
    let result = (|| -> anyhow::Result<()> {
        let mut client = Client::connect(endpoint).context("连接 daemon 失败")?;
        let snapshot = client.snapshot().context("读取 daemon 快照失败")?;
        let mut app = App::new();
        app.on_event(snapshot);
        app.run(client)
    })();
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("voime: {error:#}");
            ExitCode::from(1)
        }
    }
}

fn run_ctl(endpoint: &Endpoint, action: CtlAction) -> ExitCode {
    match run_ctl_inner(endpoint, action) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("voime: {error:#}");
            ExitCode::from(1)
        }
    }
}

fn run_ctl_inner(endpoint: &Endpoint, action: CtlAction) -> anyhow::Result<()> {
    if let CtlAction::Use { package_id } = &action {
        let root = default_model_dir();
        model::select_package(&root, package_id)?;
        println!("selected {package_id}；运行中的 daemon 需执行 restart-backend 才生效");
        return Ok(());
    }
    if matches!(action, CtlAction::Models) {
        return list_models(endpoint);
    }

    let mut client = Client::connect_as(endpoint, Role::Control).context("连接 daemon 失败")?;
    let _ = client.snapshot().context("读取 daemon 快照失败")?;

    match action {
        CtlAction::Start => {
            client.send(&ClientMessage::Start {
                segment_id: 0,
                source: TriggerSource::Cli,
            })?;
            print_state(client.wait_for_state(Duration::from_secs(3))?);
        }
        CtlAction::Stop => {
            client.send(&ClientMessage::Stop {
                segment_id: 0,
                source: TriggerSource::Cli,
            })?;
            print_state(client.wait_for_state(Duration::from_secs(3))?);
        }
        CtlAction::Toggle => {
            client.send(&ClientMessage::Toggle {
                segment_id: 0,
                source: TriggerSource::Cli,
            })?;
            print_state(client.wait_for_state(Duration::from_secs(3))?);
        }
        CtlAction::Status => {
            client.send(&ClientMessage::Status)?;
            loop {
                match client.next_event_timeout(Duration::from_secs(3))? {
                    ServerMessage::Status { status } => {
                        println!(
                            "recording={} finalizing={} segment={:?} mode={} hotkey={} addon={:?} transcript={}",
                            status.recording,
                            status.finalizing,
                            status.segment_id,
                            status
                                .mode
                                .map(|mode| mode.as_str())
                                .unwrap_or("-"),
                            status.hotkey.as_deref().unwrap_or("-"),
                            status.addon,
                            status.transcript_count
                        );
                        break;
                    }
                    ServerMessage::Error { code, message } => {
                        anyhow::bail!("daemon 错误（{code:?}）：{message}");
                    }
                    ServerMessage::Bye => anyhow::bail!("daemon 已退出"),
                    _ => {}
                }
            }
        }
        CtlAction::Clear => {
            client.send(&ClientMessage::ClearHistory)?;
            loop {
                match client.next_event_timeout(Duration::from_secs(3))? {
                    ServerMessage::HistoryCleared => {
                        println!("history cleared");
                        break;
                    }
                    ServerMessage::Error { code, message } => {
                        anyhow::bail!("daemon 错误（{code:?}）：{message}");
                    }
                    ServerMessage::Bye => anyhow::bail!("daemon 已退出"),
                    _ => {}
                }
            }
        }
        CtlAction::Models => unreachable!("handled before connecting"),
        CtlAction::Use { .. } => unreachable!("handled before connecting"),
        CtlAction::RestartBackend => {
            client.send(&ClientMessage::RestartBackend)?;
            loop {
                match client.next_event_timeout(Duration::from_secs(120))? {
                    ServerMessage::BackendState {
                        phase,
                        model: Some(model),
                        message,
                    } => match phase {
                        BackendPhase::Restarting => {
                            println!("restarting…");
                        }
                        BackendPhase::Ready => {
                            println!(
                                "ready id={} kind={} display={}",
                                model.id, model.kind, model.display_name
                            );
                            break;
                        }
                        BackendPhase::Failed => {
                            anyhow::bail!(
                                "重启后端失败：{}",
                                message.unwrap_or_else(|| "未知错误".to_string())
                            );
                        }
                    },
                    ServerMessage::Error { code, message } => {
                        anyhow::bail!("daemon 错误（{code:?}）：{message}");
                    }
                    ServerMessage::Bye => anyhow::bail!("daemon 已退出"),
                    _ => {}
                }
            }
        }
        CtlAction::Quit => {
            client.send(&ClientMessage::Quit)?;
            loop {
                match client.next_event_timeout(Duration::from_secs(3))? {
                    ServerMessage::Bye => {
                        println!("voimed stopped");
                        break;
                    }
                    ServerMessage::Error { code, message } => {
                        anyhow::bail!("daemon 错误（{code:?}）：{message}");
                    }
                    _ => {}
                }
            }
        }
    }
    Ok(())
}

fn list_models(endpoint: &Endpoint) -> anyhow::Result<()> {
    if let Ok(mut client) = Client::connect_as(endpoint, Role::Control) {
        if client.snapshot().is_ok() {
            client.send(&ClientMessage::ListModels)?;
            loop {
                match client.next_event_timeout(Duration::from_secs(5))? {
                    ServerMessage::Models { models } => {
                        print_models(&models);
                        return Ok(());
                    }
                    ServerMessage::Error { code, message } => {
                        anyhow::bail!("daemon 错误（{code:?}）：{message}");
                    }
                    ServerMessage::Bye => anyhow::bail!("daemon 已退出"),
                    _ => {}
                }
            }
        }
    }

    let root = default_model_dir();
    let active = model::read_active_id(&root).ok().flatten();
    let models: Vec<voime_ipc::ModelInfo> = model::scan_models(&root)
        .into_iter()
        .map(|package| voime_ipc::ModelInfo {
            active: active.as_deref() == Some(package.id.as_str()),
            running: false,
            id: package.id,
            display_name: package.display_name,
            kind: package.kind.as_str().to_string(),
        })
        .collect();
    println!("daemon 未运行，以下为本地安装的模型包：");
    print_models(&models);
    Ok(())
}

fn print_models(models: &[voime_ipc::ModelInfo]) {
    if models.is_empty() {
        println!("没有已安装的模型包");
        return;
    }
    for model in models {
        let active = if model.active { "*" } else { " " };
        let running = if model.running { "running" } else { "-" };
        println!(
            "{active} {:<28} {:<14} {:<10} {}",
            model.id, model.kind, running, model.display_name
        );
    }
}

fn print_state(message: ServerMessage) {
    match message {
        ServerMessage::State {
            recording,
            finalizing,
            segment_id,
        } => println!("recording={recording} finalizing={finalizing} segment={segment_id:?}"),
        ServerMessage::Error { code, message } => {
            eprintln!("voime: daemon 错误（{code:?}）：{message}");
        }
        _ => {}
    }
}
