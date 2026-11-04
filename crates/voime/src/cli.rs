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
    /// 检查端点、daemon、模型、音频与平台前端
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
    Quit,
    /// 写 Final 后端配置（不连接 daemon）
    Backend {
        backend: BackendId,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, clap::ValueEnum)]
pub enum BackendId {
    Cpu,
    Firered,
    Qwen3,
}

impl BackendId {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Cpu => "cpu",
            Self::Firered => "firered",
            Self::Qwen3 => "qwen3",
        }
    }
}

impl std::fmt::Display for BackendId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl serde::Serialize for BackendId {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> serde::Deserialize<'de> for BackendId {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = String::deserialize(deserializer)?;
        match value.trim().to_ascii_lowercase().as_str() {
            "cpu" => Ok(Self::Cpu),
            "firered" => Ok(Self::Firered),
            "qwen3" => Ok(Self::Qwen3),
            other => Err(serde::de::Error::custom(format!(
                "Final 后端必须是 cpu、firered 或 qwen3，实际 {other}"
            ))),
        }
    }
}

pub fn parse() -> Cli {
    Cli::parse()
}
