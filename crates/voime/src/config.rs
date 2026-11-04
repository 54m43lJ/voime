use std::path::PathBuf;

use anyhow::{bail, Context};
use serde::{Deserialize, Serialize};

use crate::cli::BackendId;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FinalConfig {
    pub backend: BackendId,
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
    pub fn path() -> PathBuf {
        config_dir().join("voime").join("config.toml")
    }

    pub fn load() -> anyhow::Result<Self> {
        let path = Self::path();
        let content = match std::fs::read_to_string(&path) {
            Ok(content) => content,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Self {
                    backend: BackendId::Cpu,
                })
            }
            Err(error) => {
                return Err(error).with_context(|| format!("读取配置失败 {}", path.display()))
            }
        };
        let file: ConfigFile = toml::from_str(&content)
            .with_context(|| format!("配置文件格式错误 {}", path.display()))?;
        Ok(Self {
            backend: file.final_config.backend,
        })
    }
}

/// Atomically replaces only `[final].backend`, preserving other sections.
pub fn write_backend(backend: BackendId) -> anyhow::Result<()> {
    let path = FinalConfig::path();
    let mut document: toml::Value = match std::fs::read_to_string(&path) {
        Ok(content) => toml::from_str(&content)
            .with_context(|| format!("配置文件格式错误 {}", path.display()))?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            toml::Value::Table(toml::map::Map::new())
        }
        Err(error) => {
            return Err(error).with_context(|| format!("读取配置失败 {}", path.display()))
        }
    };
    let Some(table) = document.as_table_mut() else {
        bail!("配置文件根必须是 TOML 表：{}", path.display());
    };
    let final_table = table
        .entry("final".to_string())
        .or_insert_with(|| toml::Value::Table(toml::map::Map::new()));
    let Some(final_table) = final_table.as_table_mut() else {
        bail!("配置项 final 必须是 TOML 表：{}", path.display());
    };
    final_table.insert(
        "backend".to_string(),
        toml::Value::String(backend.as_str().to_string()),
    );

    let parent = path.parent().context("配置文件路径没有父目录")?;
    std::fs::create_dir_all(parent)
        .with_context(|| format!("创建配置目录失败 {}", parent.display()))?;
    let temporary = parent.join(format!(".config.toml.tmp-{}", std::process::id()));
    let body = toml::to_string_pretty(&document).context("序列化配置失败")?;
    if let Err(error) = std::fs::write(&temporary, body) {
        let _ = std::fs::remove_file(&temporary);
        return Err(error).with_context(|| format!("写入临时配置失败 {}", temporary.display()));
    }
    if let Err(error) = std::fs::rename(&temporary, &path) {
        let _ = std::fs::remove_file(&temporary);
        return Err(error).with_context(|| format!("替换配置失败 {}", path.display()));
    }
    Ok(())
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

#[cfg(test)]
mod tests {
    #[test]
    fn preserves_unrelated_sections() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("voime").join("config.toml");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "[other]\nvalue = 1\n").unwrap();
        let document: toml::Value =
            toml::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert!(document.get("other").is_some());
    }
}
