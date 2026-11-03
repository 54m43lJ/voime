use clap::Parser;
use voime_ipc::Backend;

#[derive(Debug, Parser)]
#[command(
    name = "voimed",
    version,
    about = "Voime 本地 ASR 守护进程",
    max_term_width = 100
)]
pub struct Cli {
    /// 本次运行使用 CPU 或 GPU 后端；覆盖持久化设置
    #[arg(long, value_name = "cpu|gpu", value_parser = parse_backend)]
    pub backend: Option<Backend>,

    /// 输入设备名称，按精确字符串匹配
    #[arg(long, value_name = "NAME")]
    pub input_device: Option<String>,

    /// ONNX Runtime intra-op 线程数
    #[arg(long, value_name = "N", value_parser = parse_positive_usize)]
    pub threads: Option<usize>,

    /// 本机传输端点（Unix socket 路径或 Windows 命名管道名）
    #[arg(long, alias = "socket", value_name = "ENDPOINT")]
    pub endpoint: Option<String>,

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

fn parse_backend(value: &str) -> Result<Backend, String> {
    value.parse()
}

pub fn parse() -> Cli {
    Cli::parse()
}
