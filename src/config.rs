use std::path::PathBuf;

use anyhow::{bail, Context};

use crate::{cli, model};

#[derive(Debug)]
pub struct Config {
    pub mode: cli::Mode,
    pub model_dir: PathBuf,
    pub input_device: Option<String>,
    pub threads: usize,
    pub manifest: model::ModelManifest,
}

impl Config {
    pub fn resolve(args: cli::Args) -> anyhow::Result<Self> {
        let model_dir = args.model_dir.unwrap_or_else(default_model_dir);
        let manifest_path = model_dir.join(model::MANIFEST_FILE);
        let manifest = model::ModelManifest::load(&manifest_path)
            .with_context(|| format!("加载模型清单失败 {}", manifest_path.display()))?;
        manifest
            .verify(&model_dir)
            .with_context(|| format!("校验模型包失败 {}", model_dir.display()))?;
        let mut manifest = manifest;
        manifest.rebase(&model_dir).context("解析模型包路径失败")?;

        let threads = args.threads.unwrap_or_else(default_threads);
        if threads == 0 {
            bail!("线程数必须大于 0");
        }

        Ok(Self {
            mode: args.mode,
            model_dir,
            input_device: args.input_device,
            threads,
            manifest,
        })
    }
}

pub fn default_model_dir() -> PathBuf {
    #[cfg(windows)]
    {
        if let Some(local) = std::env::var_os("LOCALAPPDATA") {
            if !local.is_empty() {
                return PathBuf::from(local).join("voime").join("models");
            }
        }
        if let Some(profile) = std::env::var_os("USERPROFILE") {
            if !profile.is_empty() {
                return PathBuf::from(profile)
                    .join("AppData")
                    .join("Local")
                    .join("voime")
                    .join("models");
            }
        }
    }

    #[cfg(not(windows))]
    {
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
    }

    PathBuf::from("models")
}

pub fn default_threads() -> usize {
    std::thread::available_parallelism()
        .map(|count| count.get())
        .unwrap_or(1)
        .min(4)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_threads_is_positive_and_capped() {
        let threads = default_threads();
        assert!(threads >= 1);
        assert!(threads <= 4);
    }
}
