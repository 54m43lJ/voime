use std::path::{Path, PathBuf};

use anyhow::bail;

use crate::model::{load_models, ModelSet};

pub const DEFAULT_THREADS: usize = 2;

#[derive(Clone, Debug)]
pub struct Config {
    pub model_root: PathBuf,
    pub input_device: Option<String>,
    pub threads: usize,
    pub models: ModelSet,
}

#[derive(Clone, Debug, Default)]
pub struct Args {
    pub model_root: Option<PathBuf>,
    pub input_device: Option<String>,
    pub threads: Option<usize>,
    pub verbose: bool,
}

pub fn resolve(args: Args) -> anyhow::Result<Config> {
    let root = args.model_root.unwrap_or_else(default_model_root);
    resolve_at(&root, args.input_device, args.threads)
}

pub fn resolve_at(
    root: &Path,
    input_device: Option<String>,
    threads: Option<usize>,
) -> anyhow::Result<Config> {
    let threads = threads.unwrap_or(DEFAULT_THREADS);
    if threads == 0 {
        bail!("线程数必须大于 0");
    }
    let models = load_models(root)?;
    Ok(Config {
        model_root: root.to_path_buf(),
        input_device,
        threads,
        models,
    })
}

#[cfg(windows)]
pub fn default_model_root() -> PathBuf {
    if let Some(data) = std::env::var_os("LOCALAPPDATA") {
        if !data.is_empty() {
            return PathBuf::from(data).join("voime").join("models");
        }
    }
    xdg_model_root()
}

#[cfg(not(windows))]
pub fn default_model_root() -> PathBuf {
    xdg_model_root()
}

fn xdg_model_root() -> PathBuf {
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
    fn rejects_zero_threads_before_touching_disk() {
        let error = resolve_at(Path::new("/nonexistent"), None, Some(0)).unwrap_err();
        assert!(error.to_string().contains("线程数"));
    }

    #[test]
    fn default_root_ends_with_voime_models() {
        assert!(default_model_root().ends_with(Path::new("voime").join("models")));
    }

    #[test]
    fn resolve_reports_missing_model_directory() {
        let directory = tempfile::tempdir().unwrap();
        let error = resolve_at(directory.path(), None, None).unwrap_err();
        assert!(format!("{error:#}").contains("firered-vad"));
    }
}
