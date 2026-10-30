use std::path::PathBuf;

use anyhow::{bail, Context};

use crate::model::ModelManifest;

pub const DEFAULT_THREADS: usize = 2;

#[derive(Clone, Debug)]
pub struct Config {
    pub model_dir: PathBuf,
    pub input_device: Option<String>,
    pub threads: usize,
    pub socket: PathBuf,
    pub manifest: ModelManifest,
}

#[derive(Clone, Debug, Default)]
pub struct Args {
    pub model_dir: Option<PathBuf>,
    pub input_device: Option<String>,
    pub threads: Option<usize>,
    pub socket: Option<PathBuf>,
    pub verbose: bool,
}

pub fn resolve(args: Args) -> anyhow::Result<Config> {
    let model_dir = args.model_dir.unwrap_or_else(default_model_dir);
    let socket = args.socket.unwrap_or_else(default_socket);
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
        socket,
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

pub fn default_socket() -> PathBuf {
    if let Some(runtime) = std::env::var_os("XDG_RUNTIME_DIR") {
        if !runtime.is_empty() {
            return PathBuf::from(runtime).join("voime").join("voimed.sock");
        }
    }
    std::env::temp_dir()
        .join(format!("voime-{}", current_uid()))
        .join("voimed.sock")
}

fn current_uid() -> u32 {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        std::fs::metadata("/proc/self")
            .map(|metadata| metadata.uid())
            .unwrap_or(0)
    }
    #[cfg(not(unix))]
    {
        0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_socket_ends_with_documented_name() {
        let socket = default_socket();
        assert_eq!(
            socket.file_name().and_then(|name| name.to_str()),
            Some("voimed.sock")
        );
    }

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
