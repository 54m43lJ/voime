mod app;
mod cli;
mod client;
mod doctor;
mod ui;

use std::process::ExitCode;
use std::time::Duration;

use anyhow::Context;
use voime_core::config::default_socket;
use voime_ipc::{ClientMessage, Role, ServerMessage, TriggerSource};

use crate::app::App;
use crate::cli::{Cli, Command, CtlAction};
use crate::client::Client;

fn main() -> ExitCode {
    let cli = parse_cli();
    let socket = cli.socket.unwrap_or_else(default_socket);
    match cli.command {
        None => run_tui(&socket),
        Some(Command::Ctl(args)) => run_ctl(&socket, args.action),
        Some(Command::Doctor) => {
            if doctor::run(&socket) {
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

fn run_tui(socket: &std::path::Path) -> ExitCode {
    let result = (|| -> anyhow::Result<()> {
        let mut client = Client::connect(socket).context("连接 daemon 失败")?;
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

fn run_ctl(socket: &std::path::Path, action: CtlAction) -> ExitCode {
    match run_ctl_inner(socket, action) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("voime: {error:#}");
            ExitCode::from(1)
        }
    }
}

fn run_ctl_inner(socket: &std::path::Path, action: CtlAction) -> anyhow::Result<()> {
    let mut client = Client::connect_as(socket, Role::Control).context("连接 daemon 失败")?;
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
