use std::path::{Path, PathBuf};

use anyhow::{bail, Context};

use crate::model::{self, ModelManifest, ACTIVE_FILE, LEGACY_PACKAGE_ID, MANIFEST_FILE};

pub const DEFAULT_THREADS: usize = 2;

#[derive(Clone, Debug)]
pub struct Config {
    /// Fixed model root. The daemon resolves `active` below this directory.
    pub model_root: PathBuf,
    /// Package id currently resolved, or `legacy` for a pre-package install.
    pub model_id: String,
    /// Resolved, rebased package directory.
    pub model_dir: PathBuf,
    /// Set when `--model-dir` overrides the active pointer.
    pub model_dir_override: Option<PathBuf>,
    pub input_device: Option<String>,
    pub threads: usize,
    pub manifest: ModelManifest,
}

impl Config {
    pub fn override_active(&self) -> bool {
        self.model_dir_override.is_some()
    }
}

#[derive(Clone, Debug, Default)]
pub struct Args {
    pub model_dir: Option<PathBuf>,
    pub input_device: Option<String>,
    pub threads: Option<usize>,
    pub verbose: bool,
}

pub fn resolve(args: Args) -> anyhow::Result<Config> {
    let threads = args.threads.unwrap_or(DEFAULT_THREADS);
    if threads == 0 {
        bail!("线程数必须大于 0");
    }

    let (model_root, model_id, model_dir) = resolve_model_dir(args.model_dir.as_deref())?;
    let manifest_path = model_dir.join(MANIFEST_FILE);
    let mut manifest = ModelManifest::load(&manifest_path).with_context(|| {
        format!(
            "加载模型清单失败 {}（active = {}）",
            manifest_path.display(),
            model_id
        )
    })?;
    manifest.verify(&model_dir).with_context(|| {
        format!(
            "校验模型包失败 {}（active = {}）",
            model_dir.display(),
            model_id
        )
    })?;
    manifest.rebase(&model_dir).context("解析模型包路径失败")?;

    Ok(Config {
        model_root,
        model_id,
        model_dir,
        model_dir_override: args.model_dir,
        input_device: args.input_device,
        threads,
        manifest,
    })
}

/// Resolves `--model-dir` first, then `<model_root>/active`, then the legacy
/// layout where the model root itself contains `manifest.json`.
pub fn resolve_model_dir(explicit: Option<&Path>) -> anyhow::Result<(PathBuf, String, PathBuf)> {
    resolve_model_dir_at(&default_model_dir(), explicit)
}

pub fn resolve_model_dir_at(
    root: &Path,
    explicit: Option<&Path>,
) -> anyhow::Result<(PathBuf, String, PathBuf)> {
    if let Some(dir) = explicit {
        let dir = dir.to_path_buf();
        let id = dir
            .file_name()
            .and_then(|name| name.to_str())
            .filter(|name| !name.is_empty())
            .unwrap_or("explicit")
            .to_string();
        let root = dir
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| root.to_path_buf());
        return Ok((root, id, dir));
    }

    match model::read_active_id(root)? {
        Some(id) => {
            let dir = model::package_dir(root, &id)?;
            Ok((root.to_path_buf(), id, dir))
        }
        None if root.join(MANIFEST_FILE).is_file() => Ok((
            root.to_path_buf(),
            LEGACY_PACKAGE_ID.to_string(),
            root.to_path_buf(),
        )),
        None => bail!(
            "未找到模型包：{} 中既没有 {}，也没有旧布局的 {}。请先安装模型包或使用 --model-dir",
            root.display(),
            ACTIVE_FILE,
            MANIFEST_FILE
        ),
    }
}

pub fn default_model_dir() -> PathBuf {
    #[cfg(windows)]
    if let Some(data) = std::env::var_os("LOCALAPPDATA") {
        if !data.is_empty() {
            return PathBuf::from(data).join("voime").join("models");
        }
    }
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

    #[test]
    fn explicit_model_dir_wins_over_active() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        std::fs::write(root.join(ACTIVE_FILE), "other\n").unwrap();
        let package = root.join("explicit");
        std::fs::create_dir_all(&package).unwrap();
        let (resolved_root, id, dir) = resolve_model_dir(Some(package.as_path())).unwrap();
        assert_eq!(resolved_root, root);
        assert_eq!(id, "explicit");
        assert_eq!(dir, package);
    }

    #[test]
    fn missing_active_uses_legacy_layout() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        std::fs::write(root.join(MANIFEST_FILE), "{}").unwrap();
        // resolve_model_dir only checks existence; it does not parse here.
        let (resolved_root, id, dir) = resolve_model_dir_at(root, None).unwrap();
        assert_eq!(resolved_root, root);
        assert_eq!(id, LEGACY_PACKAGE_ID);
        assert_eq!(dir, root);
    }
}
