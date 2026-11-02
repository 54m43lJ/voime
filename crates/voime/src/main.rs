mod app;
mod cli;
mod client;
mod doctor;
mod ui;

use std::process::ExitCode;
use std::time::Duration;

use anyhow::Context;
use voime_ipc::{BackendPhase, ClientMessage, Role, ServerMessage, TriggerSource};
use voime_transport::{default_endpoint, Endpoint};

use crate::app::App;
use crate::cli::{Cli, Command, CtlAction};
use crate::client::Client;

const CTL_TIMEOUT: Duration = Duration::from_secs(3);
const RESTART_TIMEOUT: Duration = Duration::from_secs(120);

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
    let mut client = Client::connect_as(endpoint, Role::Control).context("连接 daemon 失败")?;
    let _ = client.snapshot().context("读取 daemon 快照失败")?;

    match action {
        CtlAction::Start => {
            client.send(&ClientMessage::Start {
                segment_id: 0,
                source: TriggerSource::Cli,
            })?;
            print_state(client.wait_for_state(CTL_TIMEOUT)?);
        }
        CtlAction::Stop => {
            client.send(&ClientMessage::Stop {
                segment_id: 0,
                source: TriggerSource::Cli,
            })?;
            print_state(client.wait_for_state(CTL_TIMEOUT)?);
        }
        CtlAction::Toggle => {
            client.send(&ClientMessage::Toggle {
                segment_id: 0,
                source: TriggerSource::Cli,
            })?;
            print_state(client.wait_for_state(CTL_TIMEOUT)?);
        }
        CtlAction::Status => {
            client.send(&ClientMessage::Status {})?;
            loop {
                match client.next_event_timeout(CTL_TIMEOUT)? {
                    ServerMessage::Status { status } => {
                        println!(
                            "recording={} finalizing={} segment={:?} mode={} hotkey={} addon={} transcript={}",
                            status.recording,
                            status.finalizing,
                            status.segment_id,
                            status.mode.map(|mode| mode.as_str()).unwrap_or("-"),
                            status.hotkey.as_deref().unwrap_or("-"),
                            frontend_state(status.addon),
                            status.transcript_count
                        );
                        break;
                    }
                    ServerMessage::Error { code, message } => {
                        anyhow::bail!("daemon 错误（{code:?}）：{message}");
                    }
                    ServerMessage::Bye {} => anyhow::bail!("daemon 已退出"),
                    _ => {}
                }
            }
        }
        CtlAction::Models => {
            client.send(&ClientMessage::ListModels {})?;
            loop {
                match client.next_event_timeout(Duration::from_secs(5))? {
                    ServerMessage::Models { models } => {
                        print_models(&models);
                        break;
                    }
                    ServerMessage::Error { code, message } => {
                        anyhow::bail!("daemon 错误（{code:?}）：{message}");
                    }
                    ServerMessage::Bye {} => anyhow::bail!("daemon 已退出"),
                    _ => {}
                }
            }
        }
        CtlAction::Use { package_id } => {
            client.send(&ClientMessage::SetModel {
                package_id: package_id.clone(),
            })?;
            loop {
                match client.next_event_timeout(CTL_TIMEOUT)? {
                    ServerMessage::ModelSelected { package_id } => {
                        println!("selected {package_id}（restart backend to apply）");
                        break;
                    }
                    ServerMessage::Error { code, message } => {
                        anyhow::bail!("daemon 错误（{code:?}）：{message}");
                    }
                    ServerMessage::Bye {} => anyhow::bail!("daemon 已退出"),
                    _ => {}
                }
            }
        }
        CtlAction::RestartBackend => {
            client.send(&ClientMessage::RestartBackend {})?;
            loop {
                match client.next_event_timeout(RESTART_TIMEOUT)? {
                    ServerMessage::BackendState {
                        phase,
                        model,
                        message,
                    } => match phase {
                        BackendPhase::Restarting => println!("backend restarting"),
                        BackendPhase::Ready => {
                            let id = model
                                .map(|model| model.id)
                                .unwrap_or_else(|| "-".to_string());
                            println!("backend ready {id}");
                            break;
                        }
                        BackendPhase::Failed => anyhow::bail!(
                            "重启后端失败：{}",
                            message.unwrap_or_else(|| "未知错误".to_string())
                        ),
                    },
                    ServerMessage::Error { code, message } => {
                        anyhow::bail!("daemon 错误（{code:?}）：{message}");
                    }
                    ServerMessage::Bye {} => anyhow::bail!("daemon 已退出"),
                    _ => {}
                }
            }
        }
        CtlAction::Clear => {
            client.send(&ClientMessage::ClearHistory {})?;
            loop {
                match client.next_event_timeout(CTL_TIMEOUT)? {
                    ServerMessage::HistoryCleared {} => {
                        println!("history cleared");
                        break;
                    }
                    ServerMessage::Error { code, message } => {
                        anyhow::bail!("daemon 错误（{code:?}）：{message}");
                    }
                    ServerMessage::Bye {} => anyhow::bail!("daemon 已退出"),
                    _ => {}
                }
            }
        }
        CtlAction::Quit => {
            client.send(&ClientMessage::Quit {})?;
            loop {
                match client.next_event_timeout(CTL_TIMEOUT)? {
                    ServerMessage::Bye {} => {
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

fn print_models(models: &[voime_ipc::ModelInfo]) {
    if models.is_empty() {
        println!("没有已安装的 Final 模型包");
        return;
    }
    for model in models {
        let device = if model.runtime.as_deref() == Some("builtin") || model.runtime.is_none() {
            "CPU"
        } else {
            "GPU"
        };
        let mut line = format!(
            "{:<24} {:<26} {:<3} available={:<5} active={:<5} running={:<5}",
            model.id, model.display_name, device, model.available, model.active, model.running
        );
        if !model.available {
            if let Some(runtime) = model.runtime.as_deref() {
                line.push_str(&format!(" runtime={runtime} unavailable"));
            }
        }
        println!("{line}");
    }
}

fn frontend_state(state: voime_ipc::FrontendState) -> &'static str {
    match state {
        voime_ipc::FrontendState::Connected => "Connected",
        voime_ipc::FrontendState::Disconnected => "Disconnected",
    }
}
