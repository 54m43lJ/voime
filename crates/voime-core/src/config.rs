use std::path::PathBuf;

use anyhow::{bail, Context};

use crate::model::ModelManifest;

pub const DEFAULT_THREADS: usize = 2;

#[derive(Clone, Debug)]
pub struct Config {
    pub model_dir: PathBuf,
    pub input_device: Option<String>,
    pub threads: usize,
    pub manifest: ModelManifest,
}

#[derive(Clone, Debug, Default)]
pub struct Args {
    pub model_dir: Option<PathBuf>,
    pub input_device: Option<String>,
    pub threads: Option<usize>,
    pub verbose: bool,
}

pub fn resolve(args: Args) -> anyhow::Result<Config> {
    let model_dir = args.model_dir.unwrap_or_else(default_model_dir);
    let threads = args.threads.unwrap_or(DEFAULT_THREADS);
    if threads == 0 {
        bail!("线程数必须大于 0");
    }

    let manifest_path = model_dir.join(crate::model::MANIFEST_FILE);
    let mut manifest = ModelManifest::load(&manifest_path)
        .with_context(|| format!("加载模型清单失败 {}", manifest_path.display()))?;
    manifest
        .verify(&model_dir)
        .with_context(|| format!("校验模型包失败 {}", model_dir.display()))?;
    manifest.rebase(&model_dir).context("解析模型包路径失败")?;

    Ok(Config {
        model_dir,
        input_device: args.input_device,
        threads,
        manifest,
    })
}

pub fn default_model_dir() -> PathBuf {
    if let Some(data) = std::env::var_os("XDG_DATA_HOME") {
        if !data.is_empty() {
            return PathBuf::from(data).join("voime").join("models");
        }
    }
    if let Some(home) = std::env::var_os("HOME") {
        if !home.is_empty() {
            return PathBuf::from(home)
                .join(".local")
                .join("share")
                .join("voime")
                .join("models");
        }
    }
    PathBuf::from("models")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_zero_threads() {
        let error = resolve(Args {
            threads: Some(0),
            ..Args::default()
        })
        .unwrap_err();
        assert!(error.to_string().contains("线程数"));
    }
}
