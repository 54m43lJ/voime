use std::fs::{self, File};
use std::io::Write;
use std::path::{Component, Path, PathBuf};

use anyhow::{bail, Context};

use crate::model::{ModelManifest, MANIFEST_FILE, PREVIEW_PACKAGE_ID};

pub const DEFAULT_THREADS: usize = 2;
pub const DEFAULT_PRE_ROLL_MS: u32 = 250;
pub const ACTIVE_FILE: &str = "active";

/// Resolved model packages. `manifest` always describes the Final package; the
/// fixed SenseVoice base package is exposed separately because it produces
/// Preview regardless of the Final selection.
#[derive(Clone, Debug)]
pub struct Config {
    pub model_root: PathBuf,
    pub model_id: String,
    pub model_dir: PathBuf,
    pub manifest: ModelManifest,
    pub preview_dir: PathBuf,
    pub preview_manifest: ModelManifest,
    pub input_device: Option<String>,
    pub threads: usize,
    pub preview_enabled: bool,
    pub pre_roll_ms: u32,
    /// True when `--model-dir` fixed the Final package. Model switching is
    /// rejected in that mode.
    pub explicit_final: bool,
}

#[derive(Clone, Debug, Default)]
pub struct Args {
    /// Only overrides the Final package directory. The SenseVoice base package
    /// is still resolved below the default model root.
    pub model_dir: Option<PathBuf>,
    pub input_device: Option<String>,
    pub threads: Option<usize>,
    pub verbose: bool,
}

pub fn resolve(args: Args) -> anyhow::Result<Config> {
    resolve_at(&default_model_root(), args)
}

/// Resolves the configuration against an explicit model root. Kept public for
/// integration tests, `voimed` restart-backend and `voime doctor`.
pub fn resolve_at(root: &Path, args: Args) -> anyhow::Result<Config> {
    let threads = args.threads.unwrap_or(DEFAULT_THREADS);
    if threads == 0 {
        bail!("线程数必须大于 0");
    }

    let model_root = root.to_path_buf();
    let (model_id, model_dir) = match args.model_dir.as_deref() {
        Some(dir) => {
            let id = dir
                .file_name()
                .and_then(|name| name.to_str())
                .filter(|name| !name.is_empty())
                .unwrap_or("explicit")
                .to_string();
            (id, dir.to_path_buf())
        }
        None => {
            let id = read_active_id(&model_root)?.with_context(|| {
                format!(
                    "未找到 Final 选择：{}（请先运行安装脚本或使用 --model-dir）",
                    model_root.join(ACTIVE_FILE).display()
                )
            })?;
            let dir = resolve_package(&model_root, &id)?;
            (id, dir)
        }
    };

    let preview_dir = resolve_package(&model_root, PREVIEW_PACKAGE_ID).with_context(|| {
        format!(
            "SenseVoice 基础包缺失：{}（Preview 与 CPU Final 都需要它）",
            model_root.join(PREVIEW_PACKAGE_ID).display()
        )
    })?;
    let preview_manifest = load_verified(&preview_dir, "SenseVoice 基础包")?;
    let manifest = load_verified(&model_dir, "Final 包")?;
    for required in &manifest.requires {
        let required_dir = resolve_package(&model_root, required)?;
        if !required_dir.join(MANIFEST_FILE).is_file() {
            bail!(
                "Final 包依赖缺失：{}（{}）",
                required,
                required_dir.display()
            );
        }
    }

    Ok(Config {
        model_root,
        model_id,
        model_dir,
        manifest,
        preview_dir,
        preview_manifest,
        input_device: args.input_device,
        threads,
        preview_enabled: true,
        pre_roll_ms: DEFAULT_PRE_ROLL_MS,
        explicit_final: args.model_dir.is_some(),
    })
}

fn load_verified(dir: &Path, label: &str) -> anyhow::Result<ModelManifest> {
    let path = dir.join(MANIFEST_FILE);
    let mut manifest = ModelManifest::load(&path)
        .with_context(|| format!("加载{label}清单失败 {}", path.display()))?;
    manifest
        .verify(dir)
        .with_context(|| format!("校验{label}失败 {}", dir.display()))?;
    manifest
        .rebase(dir)
        .with_context(|| format!("解析{label}路径失败 {}", dir.display()))?;
    Ok(manifest)
}

pub fn default_model_root() -> PathBuf {
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

pub fn validate_package_id(package_id: &str) -> anyhow::Result<()> {
    if package_id.is_empty() {
        bail!("模型包 id 不能为空");
    }
    if package_id == "." || package_id == ".." {
        bail!("模型包 id 非法：{package_id}");
    }
    if package_id.chars().any(char::is_control) {
        bail!("模型包 id 不能包含控制字符");
    }
    let path = Path::new(package_id);
    if path.is_absolute() {
        bail!("模型包 id 必须是相对目录名：{package_id}");
    }
    let mut components = path.components();
    match (components.next(), components.next()) {
        (Some(Component::Normal(_)), None) => Ok(()),
        _ => bail!("模型包 id 必须是单层目录名：{package_id}"),
    }
}

pub fn resolve_package(root: &Path, package_id: &str) -> anyhow::Result<PathBuf> {
    validate_package_id(package_id)?;
    Ok(root.join(package_id))
}

/// Reads `<root>/active`. A missing file is not an error here; callers decide
/// whether that means "first install" or a fatal configuration error.
pub fn read_active_id(root: &Path) -> anyhow::Result<Option<String>> {
    let path = root.join(ACTIVE_FILE);
    let content = match fs::read(&path) {
        Ok(content) => content,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(error).with_context(|| format!("读取 active 指针失败 {}", path.display()));
        }
    };
    let text = String::from_utf8(content)
        .with_context(|| format!("active 指针不是 UTF-8：{}", path.display()))?;
    let text = text.trim_start_matches('\u{feff}').trim();
    if text.is_empty() {
        return Ok(None);
    }
    if text.chars().any(|character| character.is_control()) {
        bail!("active 指针只能包含单行包名：{}", path.display());
    }
    validate_package_id(text)?;
    Ok(Some(text.to_string()))
}

/// Atomically replaces `<root>/active` with `package_id`.
pub fn write_active_id(root: &Path, package_id: &str) -> anyhow::Result<()> {
    validate_package_id(package_id)?;
    fs::create_dir_all(root).with_context(|| format!("创建模型根目录失败 {}", root.display()))?;
    let path = root.join(ACTIVE_FILE);
    let temporary = root.join(format!(".{ACTIVE_FILE}.tmp-{}", std::process::id()));
    let write = (|| -> anyhow::Result<()> {
        let mut file = File::create(&temporary)
            .with_context(|| format!("创建临时 active 文件失败 {}", temporary.display()))?;
        file.write_all(package_id.as_bytes())
            .with_context(|| format!("写入临时 active 文件失败 {}", temporary.display()))?;
        file.write_all(b"\n")
            .with_context(|| format!("写入临时 active 文件失败 {}", temporary.display()))?;
        file.sync_all()
            .with_context(|| format!("同步临时 active 文件失败 {}", temporary.display()))?;
        Ok(())
    })();
    if let Err(error) = write {
        let _ = fs::remove_file(&temporary);
        return Err(error);
    }
    fs::rename(&temporary, &path).with_context(|| {
        let _ = fs::remove_file(&temporary);
        format!("替换 active 指针失败 {}", path.display())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use sha2::{Digest, Sha256};

    fn write_manifest(dir: &Path, name: &str) -> String {
        fs::create_dir_all(dir).unwrap();
        let vad = b"vad";
        let model = b"model";
        let tokens = b"tokens";
        fs::write(dir.join("vad.onnx"), vad).unwrap();
        fs::write(dir.join("asr.onnx"), model).unwrap();
        fs::write(dir.join("tokens.txt"), tokens).unwrap();
        let sha = |data: &[u8]| format!("{:x}", Sha256::digest(data));
        fs::write(
            dir.join(MANIFEST_FILE),
            format!(
                r#"{{
                    "schema": 3, "display_name": "{name}", "sample_rate": 16000,
                    "fbank": {{"num_bins": 80, "frame_ms": 25, "shift_ms": 10, "window_type": "hamming"}},
                    "max_segment_ms": 20000,
                    "vad": {{"model": "vad.onnx", "sha256": "{}", "inputs": [{{"name":"x","dtype":"float32","shape":[1,-1,80]}}],
                             "outputs": [{{"name":"y","dtype":"float32","shape":[1,-1,1]}}]}},
                    "backend": {{"kind": "builtin"}},
                    "asr": {{"kind": "sense-voice", "model": {{"model": "asr.onnx", "sha256": "{}",
                             "inputs": [{{"name":"x","dtype":"float32","shape":[1,-1,560]}}],
                             "outputs": [{{"name":"y","dtype":"float32","shape":[1,-1,25055]}}]}},
                             "tokens": "tokens.txt", "token_count": 25055,
                             "options": {{"use_itn": true, "punc": "builtin"}}}}
                }}"#,
                sha(vad),
                sha(model)
            ),
        )
        .unwrap();
        sha(tokens)
    }

    #[test]
    fn rejects_zero_threads() {
        let error = resolve_at(
            Path::new("/nonexistent"),
            Args {
                threads: Some(0),
                ..Args::default()
            },
        )
        .unwrap_err();
        assert!(error.to_string().contains("线程数"));
    }

    #[test]
    fn active_pointer_accepts_bom_and_whitespace() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        assert_eq!(read_active_id(root).unwrap(), None);
        fs::write(root.join(ACTIVE_FILE), "\u{feff} sense-voice \n").unwrap();
        assert_eq!(
            read_active_id(root).unwrap().as_deref(),
            Some("sense-voice")
        );
        fs::write(root.join(ACTIVE_FILE), "one\ntwo\n").unwrap();
        assert!(read_active_id(root).is_err());
        fs::write(root.join(ACTIVE_FILE), "../escape\n").unwrap();
        assert!(read_active_id(root).is_err());
    }

    #[test]
    fn active_selects_final_but_preview_stays_fixed() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        write_manifest(&root.join(PREVIEW_PACKAGE_ID), "preview");
        let final_dir = root.join("final-package");
        write_manifest(&final_dir, "final");
        fs::write(root.join(ACTIVE_FILE), "final-package\n").unwrap();
        let config = resolve_at(root, Args::default()).unwrap();
        assert_eq!(config.model_dir, final_dir);
        assert_eq!(config.preview_dir, root.join(PREVIEW_PACKAGE_ID));
        assert_eq!(config.preview_manifest.display_name, "preview");
        assert_eq!(config.manifest.display_name, "final");
    }

    #[test]
    fn explicit_model_dir_keeps_default_root_for_preview() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("root");
        let explicit = directory.path().join("explicit");
        write_manifest(&root.join(PREVIEW_PACKAGE_ID), "preview");
        write_manifest(&explicit, "final");
        let config = resolve_at(
            &root,
            Args {
                model_dir: Some(explicit.clone()),
                ..Args::default()
            },
        )
        .unwrap();
        assert_eq!(config.model_dir, explicit);
        assert_eq!(config.preview_dir, root.join(PREVIEW_PACKAGE_ID));
        assert!(config.explicit_final);
    }
}
