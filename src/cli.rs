use std::path::PathBuf;

use clap::{Parser, ValueEnum};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, ValueEnum)]
pub enum Mode {
    #[default]
    Ptt,
    Continuous,
}

#[derive(Debug, Parser)]
#[command(
    name = "voime",
    version,
    about = "纯本地语音输入法：说话、识别、自动复制",
    max_term_width = 100
)]
pub struct Args {
    /// 运行模式：ptt 为按键说话，continuous 为持续监听
    #[arg(long, value_enum, default_value_t = Mode::Ptt)]
    pub mode: Mode,

    /// 模型包目录（包含 manifest.json）
    #[arg(long, value_name = "DIR")]
    pub model_dir: Option<PathBuf>,

    /// 输入设备名称，按精确字符串匹配
    #[arg(long, value_name = "NAME")]
    pub input_device: Option<String>,

    /// ONNX Runtime intra-op 线程数
    #[arg(long, value_name = "N", value_parser = parse_positive_usize)]
    pub threads: Option<usize>,
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

pub fn parse() -> Args {
    Args::parse()
}
