use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context};

const ACTIVE_FILE: &str = "active";
const MANIFEST_FILE: &str = "manifest.json";

#[derive(Clone, Debug)]
pub struct ModelOption {
    pub id: String,
    pub label: String,
    pub kind: String,
}

pub fn model_root() -> PathBuf {
    std::env::var_os("LOCALAPPDATA")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
        .join("voime")
        .join("models")
}

pub fn read_active() -> anyhow::Result<Option<String>> {
    read_active_at(&model_root())
}

pub fn write_active(package_id: &str) -> anyhow::Result<()> {
    write_active_at(&model_root(), package_id)
}

pub fn scan() -> Vec<ModelOption> {
    scan_at(&model_root())
}

fn read_active_at(root: &Path) -> anyhow::Result<Option<String>> {
    let path = root.join(ACTIVE_FILE);
    let content = match fs::read(&path) {
        Ok(content) => content,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(error).with_context(|| format!("读取模型选择失败 {}", path.display()));
        }
    };
    let text = String::from_utf8(content).context("active 指针不是 UTF-8")?;
    let text = text.trim_start_matches('\u{feff}').trim();
    if text.is_empty() {
        return Ok(None);
    }
    if text.chars().any(char::is_control) {
        bail!("active 指针只能包含单行包名");
    }
    Ok(Some(text.to_string()))
}

fn write_active_at(root: &Path, package_id: &str) -> anyhow::Result<()> {
    validate_id(package_id)?;
    fs::create_dir_all(root)
        .with_context(|| format!("创建模型目录失败 {}", root.display()))?;
    let path = root.join(ACTIVE_FILE);
    let temporary = root.join(format!(".{ACTIVE_FILE}.tmp-{}", std::process::id()));
    fs::write(&temporary, format!("{package_id}\n"))
        .with_context(|| format!("写入模型选择失败 {}", temporary.display()))?;
    fs::rename(&temporary, &path).with_context(|| {
        let _ = fs::remove_file(&temporary);
        format!("替换模型选择失败 {}", path.display())
    })
}

fn scan_at(root: &Path) -> Vec<ModelOption> {
    let mut models = Vec::new();
    let Ok(entries) = fs::read_dir(root) else {
        return models;
    };
    for entry in entries.flatten() {
        let dir = entry.path();
        if !dir.is_dir() {
            continue;
        }
        let Some(id) = dir.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if validate_id(id).is_err() || !dir.join(MANIFEST_FILE).is_file() {
            continue;
        }
        if let Some(option) = read_package(&dir, id) {
            models.push(option);
        }
    }
    models.sort_by(|left, right| {
        left.label
            .to_lowercase()
            .cmp(&right.label.to_lowercase())
            .then_with(|| left.id.cmp(&right.id))
    });
    models
}

fn read_package(dir: &Path, id: &str) -> Option<ModelOption> {
    let bytes = fs::read(dir.join(MANIFEST_FILE)).ok()?;
    let manifest: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
    let display_name = manifest
        .get("display_name")
        .and_then(|value| value.as_str())
        .filter(|value| !value.trim().is_empty())
        .unwrap_or(id);
    let kind = manifest
        .pointer("/asr/kind")
        .and_then(|value| value.as_str())
        .unwrap_or("unknown");
    let device = if manifest
        .pointer("/backend/kind")
        .and_then(|value| value.as_str())
        == Some("worker")
    {
        let runtime = manifest
            .pointer("/backend/runtime")
            .and_then(|value| value.as_str())
            .unwrap_or("GPU");
        format!("GPU {runtime}")
    } else {
        "CPU".to_string()
    };
    Some(ModelOption {
        id: id.to_string(),
        label: format!("{display_name}  ({kind}, {device})"),
        kind: kind.to_string(),
    })
}

fn validate_id(package_id: &str) -> anyhow::Result<()> {
    if package_id.is_empty()
        || package_id == "."
        || package_id == ".."
        || package_id.chars().any(char::is_control)
        || package_id.contains('/')
        || package_id.contains('\\')
    {
        bail!("模型包 id 非法：{package_id}");
    }
    Ok(())
}
