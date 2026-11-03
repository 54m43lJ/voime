use std::path::PathBuf;

use clap::Parser;

#[derive(Debug, Parser)]
#[command(
    name = "voime-gpu",
    version,
    about = "Voime 可选 GPU 后端 worker（FireRedASR）",
    max_term_width = 100
)]
pub struct Args {
    /// FireRed AED 包目录（包含 manifest.json）
    #[arg(long, value_name = "DIR")]
    pub manifest: PathBuf,

    /// auto 或 runtime 支持的设备名 / id
    #[arg(long, default_value = "auto", value_name = "DEVICE")]
    pub device: String,

    /// 诊断输出到 stderr；stdout 仍只用于协议帧
    #[arg(long)]
    pub verbose: bool,
}

pub fn parse() -> Args {
    Args::parse()
}
