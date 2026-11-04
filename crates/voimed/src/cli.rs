use std::path::PathBuf;

use clap::Parser;

#[derive(Debug, Clone, Parser)]
#[command(name = "voimed", version, about = "Voime 本地语音守护进程")]
pub struct Cli {
    /// 模型根目录（一模型一目录）
    #[arg(long, value_name = "DIR")]
    pub model_dir: Option<PathBuf>,

    /// 输入设备名称（精确匹配）
    #[arg(long, value_name = "NAME")]
    pub input_device: Option<String>,

    /// ONNX Runtime intra-op 线程数
    #[arg(long, value_name = "N", value_parser = parse_positive_usize)]
    pub threads: Option<usize>,

    /// 本机传输端点（socket 路径或命名管道名）
    #[arg(long, alias = "socket", value_name = "ENDPOINT")]
    pub endpoint: Option<String>,

    /// 输出启动与运行期诊断到 stderr
    #[arg(long)]
    pub verbose: bool,
}

pub fn parse() -> Cli {
    Cli::parse()
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
