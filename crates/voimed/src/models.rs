use std::collections::HashSet;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context};
use voime_core::config::{self, ACTIVE_FILE};
use voime_core::model::{BackendKind, ModelManifest, MANIFEST_FILE};
use voime_ipc::ModelInfo;

/// One installed Final package discovered below the model root.
#[derive(Clone, Debug)]
pub struct ModelEntry {
    pub id: String,
    pub display_name: String,
    pub kind: String,
    pub runtime: Option<String>,
    pub device: Option<String>,
    pub available: bool,
    pub dir: PathBuf,
    pub requires: Vec<String>,
}

pub struct ModelCatalog {
    root: PathBuf,
    active: Option<String>,
    candidates: Vec<ModelEntry>,
}

impl ModelCatalog {
    pub fn scan(root: &Path) -> anyhow::Result<Self> {
        let active = read_active(root).ok().flatten();
        let mut candidates = Vec::new();
        let entries = match std::fs::read_dir(root) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Self {
                    root: root.to_path_buf(),
                    active,
                    candidates,
                });
            }
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("读取模型根目录失败 {}", root.display()));
            }
        };
        let mut seen = HashSet::new();
        for entry in entries.flatten() {
            let dir = entry.path();
            if !dir.is_dir() {
                continue;
            }
            let Some(id) = dir.file_name().and_then(|name| name.to_str()) else {
                continue;
            };
            if config::validate_package_id(id).is_err() || !seen.insert(id.to_string()) {
                continue;
            }
            let manifest_path = dir.join(MANIFEST_FILE);
            if !manifest_path.is_file() {
                continue;
            }
            let Ok(manifest) = ModelManifest::load(&manifest_path) else {
                continue;
            };
            candidates.push(entry_from_manifest(id.to_string(), &dir, &manifest));
        }
        candidates.sort_by(|left, right| {
            left.display_name
                .to_lowercase()
                .cmp(&right.display_name.to_lowercase())
                .then_with(|| left.id.cmp(&right.id))
        });
        Ok(Self {
            root: root.to_path_buf(),
            active,
            candidates,
        })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn active(&self) -> Option<&ModelEntry> {
        let id = self.active.as_deref()?;
        self.candidates.iter().find(|entry| entry.id == id)
    }

    pub fn active_id(&self) -> Option<&str> {
        self.active.as_deref()
    }

    pub fn list(&self) -> Vec<ModelInfo> {
        self.candidates
            .iter()
            .map(|entry| ModelInfo {
                id: entry.id.clone(),
                display_name: entry.display_name.clone(),
                kind: entry.kind.clone(),
                runtime: entry.runtime.clone(),
                device: entry.device.clone(),
                available: entry.available,
                active: self.active.as_deref() == Some(entry.id.as_str()),
                running: false,
            })
            .collect()
    }

    pub fn find(&self, package_id: &str) -> Option<&ModelEntry> {
        self.candidates.iter().find(|entry| entry.id == package_id)
    }

    /// Persists a selection. Full SHA verification happens in `select_package`.
    pub fn set_active(&mut self, package_id: &str) -> anyhow::Result<()> {
        select_package(&self.root, package_id)?;
        self.active = Some(package_id.to_string());
        Ok(())
    }

    pub fn refresh(&mut self) -> anyhow::Result<()> {
        *self = Self::scan(&self.root)?;
        Ok(())
    }
}

fn entry_from_manifest(id: String, dir: &Path, manifest: &ModelManifest) -> ModelEntry {
    ModelEntry {
        id,
        display_name: manifest.display_name.clone(),
        kind: manifest.asr.kind.as_str().to_string(),
        runtime: manifest.backend.runtime.clone(),
        device: manifest.backend.device.clone(),
        available: match manifest.backend.kind {
            BackendKind::Builtin => true,
            BackendKind::Worker => worker_available(&manifest.backend.runtime),
        },
        dir: dir.to_path_buf(),
        requires: manifest.requires.clone(),
    }
}

/// Cheap availability preflight. It never creates a GPU session or loads a
/// model: it only checks that a worker binary and a runtime directory are
/// present.
pub fn worker_available(runtime: &Option<String>) -> bool {
    let runtime_ok = std::env::var_os("VOIME_GPU_RUNTIME_DIR")
        .map(|dir| !dir.is_empty() && Path::new(&dir).exists())
        .unwrap_or(false)
        || runtime
            .as_deref()
            .and_then(runtime_directory)
            .map(|dir| dir.exists())
            .unwrap_or(false);
    if !runtime_ok {
        return false;
    }
    worker_binary().is_some()
}

fn runtime_directory(runtime: &str) -> Option<PathBuf> {
    let base = std::env::var_os("VOIME_RUNTIME_DIR")?;
    Some(PathBuf::from(base).join(runtime))
}

pub fn worker_binary() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("VOIME_GPU_BIN") {
        let path = PathBuf::from(path);
        if path.is_file() {
            return Some(path);
        }
    }
    let exe = std::env::current_exe().ok()?;
    let name = if cfg!(windows) {
        "voime-gpu.exe"
    } else {
        "voime-gpu"
    };
    let sibling = exe.parent()?.join(name);
    sibling.is_file().then_some(sibling)
}

pub fn read_active(root: &Path) -> anyhow::Result<Option<String>> {
    config::read_active_id(root)
}

pub fn write_active(root: &Path, package_id: &str) -> anyhow::Result<()> {
    config::write_active_id(root, package_id)
}

pub fn resolve_package(root: &Path, package_id: &str) -> anyhow::Result<PathBuf> {
    config::resolve_package(root, package_id)
}

/// Validates structure, SHA-256, requires and then writes `active`.
pub fn select_package(root: &Path, package_id: &str) -> anyhow::Result<()> {
    config::validate_package_id(package_id)?;
    let dir = resolve_package(root, package_id)?;
    let manifest_path = dir.join(MANIFEST_FILE);
    let manifest = ModelManifest::load(&manifest_path)
        .with_context(|| format!("模型包清单无效：{}", manifest_path.display()))?;
    manifest
        .verify(&dir)
        .with_context(|| format!("模型包校验失败：{package_id}"))?;
    for required in &manifest.requires {
        let required_dir = resolve_package(root, required)?;
        if !required_dir.join(MANIFEST_FILE).is_file() {
            bail!("模型包 {package_id} 缺少依赖 {required}");
        }
    }
    config::write_active_id(root, package_id)
}

/// Loads and verifies the package currently pointed at by `active`.
pub fn active_manifest(root: &Path) -> anyhow::Result<(String, PathBuf, ModelManifest)> {
    let Some(id) = read_active(root)? else {
        bail!("{} 中没有 active 指针", root.join(ACTIVE_FILE).display());
    };
    let dir = resolve_package(root, &id)?;
    let mut manifest = ModelManifest::load(&dir.join(MANIFEST_FILE))?;
    manifest.verify(&dir)?;
    manifest.rebase(&dir)?;
    Ok((id, dir, manifest))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn worker_availability_requires_runtime_and_binary() {
        assert!(!worker_available(&Some("onnx-migraphx".to_string())));
    }

    fn write_package(root: &Path, id: &str, display_name: &str) {
        let dir = root.join(id);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join(MANIFEST_FILE),
            format!(
                r#"{{
                    "schema": 3, "display_name": "{display_name}", "sample_rate": 16000,
                    "fbank": {{"num_bins": 80, "frame_ms": 25, "shift_ms": 10, "window_type": "hamming"}},
                    "max_segment_ms": 20000,
                    "vad": {{"model": "vad.onnx", "sha256": "{}",
                             "inputs": [{{"name":"x","dtype":"float32","shape":[1,-1,80]}}],
                             "outputs": [{{"name":"y","dtype":"float32","shape":[1,-1,1]}}]}},
                    "backend": {{"kind": "builtin"}},
                    "asr": {{"kind": "sense-voice", "model": {{"model": "asr.onnx", "sha256": "{}",
                             "inputs": [{{"name":"x","dtype":"float32","shape":[1,-1,560]}}],
                             "outputs": [{{"name":"y","dtype":"float32","shape":[1,-1,25055]}}]}},
                             "tokens": "tokens.txt", "token_count": 25055,
                             "options": {{"use_itn": true, "punc": "builtin"}}}}
                }}"#,
                "0".repeat(64),
                "0".repeat(64)
            ),
        )
        .unwrap();
    }

    #[test]
    fn scans_active_and_lists_builtin_model() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        write_package(root, "sense-voice-2024-int8", "SenseVoice");
        write_package(root, "other", "Other");
        std::fs::write(root.join(ACTIVE_FILE), "other\n").unwrap();
        let catalog = ModelCatalog::scan(root).unwrap();
        assert_eq!(catalog.active_id(), Some("other"));
        let models = catalog.list();
        assert_eq!(models.len(), 2);
        assert!(models.iter().all(|model| model.available));
        let active = models.iter().find(|model| model.active).unwrap();
        assert_eq!(active.id, "other");
    }
}
