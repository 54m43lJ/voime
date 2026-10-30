use std::path::PathBuf;

use clap::{Args, Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(
    name = "voime",
    version,
    about = "Voime TUI、控制与诊断客户端",
    max_term_width = 100
)]
pub struct Cli {
    /// IPC socket 路径
    #[arg(long, global = true, value_name = "PATH")]
    pub socket: Option<PathBuf>,

    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// 一次性控制客户端
    Ctl(CtlArgs),
    /// 检查 socket、daemon、模型、音频与 Fcitx addon
    Doctor,
}

#[derive(Debug, Args)]
pub struct CtlArgs {
    #[command(subcommand)]
    pub action: CtlAction,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Subcommand)]
pub enum CtlAction {
    Start,
    Stop,
    Toggle,
    Status,
    Clear,
    Quit,
}

pub fn parse() -> Cli {
    Cli::parse()
}
