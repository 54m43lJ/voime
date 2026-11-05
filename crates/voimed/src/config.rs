use std::path::{Path, PathBuf};

use anyhow::Context;
use serde::{Deserialize, Serialize};

use crate::backend::BackendId;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FinalConfig {
    pub backend: BackendId,
}

#[derive(Clone, Debug)]
pub struct Config {
    pub final_config: FinalConfig,
    pub model_dir: PathBuf,
    pub input_device: Option<String>,
    pub threads: usize,
    pub endpoint: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, Default)]
struct ConfigFile {
    #[serde(default)]
    #[serde(rename = "final")]
    final_config: FinalSection,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct FinalSection {
    #[serde(default = "default_backend")]
    backend: BackendId,
}

impl Default for FinalSection {
    fn default() -> Self {
        Self {
            backend: BackendId::Cpu,
        }
    }
}

fn default_backend() -> BackendId {
    BackendId::Cpu
}

impl FinalConfig {
    pub fn load() -> anyhow::Result<Self> {
        read_toml(&Self::path())
    }

    pub fn path() -> PathBuf {
        config_dir().join("voime").join("config.toml")
    }
}

impl Default for FinalConfig {
    fn default() -> Self {
        Self {
            backend: BackendId::Cpu,
        }
    }
}

pub fn load() -> anyhow::Result<Config> {
    let final_config = FinalConfig::load()?;
    Ok(Config {
        final_config,
        model_dir: voime_core::config::default_model_root(),
        input_device: None,
        threads: voime_core::config::DEFAULT_THREADS,
        endpoint: None,
    })
}

pub fn from_cli(cli: &crate::cli::Cli) -> anyhow::Result<Config> {
    let final_config = FinalConfig::load()?;
    Ok(Config {
        final_config,
        model_dir: cli
            .model_dir
            .clone()
            .unwrap_or_else(voime_core::config::default_model_root),
        input_device: cli.input_device.clone(),
        threads: cli.threads.unwrap_or(voime_core::config::DEFAULT_THREADS),
        endpoint: cli.endpoint.clone(),
    })
}

fn read_toml(path: &Path) -> anyhow::Result<FinalConfig> {
    let content = match std::fs::read_to_string(path) {
        Ok(content) => content,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(FinalConfig::default())
        }
        Err(error) => {
            return Err(error).with_context(|| format!("读取配置失败 {}", path.display()))
        }
    };
    let file: ConfigFile =
        toml::from_str(&content).with_context(|| format!("配置文件格式错误 {}", path.display()))?;
    Ok(FinalConfig {
        backend: file.final_config.backend,
    })
}

#[cfg(windows)]
fn config_dir() -> PathBuf {
    std::env::var_os("APPDATA")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

#[cfg(not(windows))]
fn config_dir() -> PathBuf {
    if let Some(config) = std::env::var_os("XDG_CONFIG_HOME") {
        if !config.is_empty() {
            return PathBuf::from(config);
        }
    }
    std::env::var_os("HOME")
        .filter(|value| !value.is_empty())
        .map(|home| PathBuf::from(home).join(".config"))
        .unwrap_or_else(|| PathBuf::from("."))
}

pub fn invalid_backend_message(value: &str) -> anyhow::Error {
    anyhow::anyhow!("final.backend 值非法：{value}；合法值为 cpu、qwen3")
}

#[allow(dead_code)]
fn validate_backend(value: &str) -> anyhow::Result<BackendId> {
    BackendId::parse(value).map_err(|_| invalid_backend_message(value))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_file_means_cpu() {
        let directory = tempfile::tempdir().unwrap();
        let config = read_toml(&directory.path().join("missing.toml")).unwrap();
        assert_eq!(config.backend, BackendId::Cpu);
    }

    #[test]
    fn parses_and_rejects_backend_values() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("config.toml");
        std::fs::write(&path, "[final]\nbackend = \"qwen3\"\n").unwrap();
        assert_eq!(read_toml(&path).unwrap().backend, BackendId::Qwen3);
        std::fs::write(&path, "[final]\nbackend = \"gpu\"\n").unwrap();
        assert!(read_toml(&path).is_err());
        std::fs::write(&path, "[final]\nbackend = \"npu\"\n").unwrap();
        let error = read_toml(&path).unwrap_err();
        assert!(format!("{error:#}").contains("cpu"));
    }
}
