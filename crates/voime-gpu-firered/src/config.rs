use anyhow::{bail, Context};
use clap::{Parser, ValueEnum};

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
pub enum OrtEp {
    Cuda,
    Migraphx,
    Rocm,
    Directml,
}

impl OrtEp {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Cuda => "cuda",
            Self::Migraphx => "migraphx",
            Self::Rocm => "rocm",
            Self::Directml => "directml",
        }
    }

    pub fn device_prefix(self) -> &'static str {
        match self {
            Self::Cuda => "cuda",
            Self::Migraphx => "migraphx",
            Self::Rocm => "rocm",
            Self::Directml => "dml",
        }
    }

    /// The execution provider compiled into this worker binary.
    pub fn current() -> Self {
        if cfg!(feature = "cuda") {
            Self::Cuda
        } else if cfg!(feature = "migraphx") {
            Self::Migraphx
        } else if cfg!(feature = "rocm") {
            Self::Rocm
        } else if cfg!(feature = "directml") {
            Self::Directml
        } else {
            Self::Cuda
        }
    }
}

#[derive(Debug, Parser)]
#[command(name = "voime-gpu-firered", version)]
pub struct Args {
    #[arg(long, value_enum)]
    pub runtime: Option<OrtEp>,
    #[arg(long, default_value = "info")]
    pub log_level: String,
}

pub fn parse() -> anyhow::Result<Args> {
    let mut args = Args::parse();
    args.runtime = Some(args.runtime.unwrap_or_else(OrtEp::current));
    if args.log_level.trim().is_empty() {
        bail!("--log-level 不能为空");
    }
    Ok(args)
}

#[allow(dead_code)]
fn require_directory(path: &std::path::Path) -> anyhow::Result<()> {
    if !path.is_dir() {
        bail!("模型目录不存在：{}", path.display());
    }
    Ok(())
}

#[allow(dead_code)]
fn context_path(path: &std::path::Path) -> anyhow::Result<String> {
    path.to_str()
        .map(str::to_string)
        .with_context(|| format!("路径不是 UTF-8：{}", path.display()))
}
