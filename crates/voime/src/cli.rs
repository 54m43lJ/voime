use clap::{Args, Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(
    name = "voime",
    version,
    about = "Voime TUI、控制与诊断客户端",
    max_term_width = 100
)]
pub struct Cli {
    /// 本机传输端点（Unix socket 路径或 Windows 命名管道名）
    #[arg(long, alias = "socket", global = true, value_name = "ENDPOINT")]
    pub endpoint: Option<String>,

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

#[derive(Clone, Debug, Eq, PartialEq, Subcommand)]
pub enum CtlAction {
    Start,
    Stop,
    Toggle,
    Status,
    Clear,
    /// List installed model packages and their active / running state.
    Models,
    /// Select a model package (writes `active`; restart-backend applies it).
    Use {
        package_id: String,
    },
    /// Reload the active model package without restarting the daemon.
    RestartBackend,
    Quit,
}

pub fn parse() -> Cli {
    Cli::parse()
}
