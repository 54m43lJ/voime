use std::path::PathBuf;

use clap::Parser;

#[derive(Debug, Parser)]
#[command(
    name = "voimed",
    version,
    about = "Voime 本地 ASR 守护进程",
    max_term_width = 100
)]
pub struct Cli {
    /// 模型包目录（包含 manifest.json）
    #[arg(long, value_name = "DIR")]
    pub model_dir: Option<PathBuf>,

    /// 输入设备名称，按精确字符串匹配
    #[arg(long, value_name = "NAME")]
    pub input_device: Option<String>,

    /// ONNX Runtime intra-op 线程数
    #[arg(long, value_name = "N", value_parser = parse_positive_usize)]
    pub threads: Option<usize>,

    /// IPC socket 路径
    #[arg(long, value_name = "PATH")]
    pub socket: Option<PathBuf>,

    /// 输出启动与运行期诊断
    #[arg(long)]
    pub verbose: bool,
}

fn parse_positive_usize(value: &str) -> Result<usize, String> {
    let parsed = value
        .parse::<usize>()
        .map_err(|_| format!("线程数必须是正整数：{value}"))?;
    if parsed == 0 {
        return Err("线程数必须大于 0".to_string());
    }
    Ok(parsed)
}

pub fn parse() -> Cli {
    Cli::parse()
}
