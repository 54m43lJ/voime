use std::collections::HashSet;
use std::fs::File;
use std::io::{BufReader, Read};
use std::path::{Component, Path, PathBuf};

use anyhow::{bail, Context};
use serde::Deserialize;
use sha2::{Digest, Sha256};

pub const MANIFEST_FILE: &str = "manifest.json";
pub const SCHEMA_VERSION: u32 = 5;
pub const SAMPLES_PER_SECOND: u32 = 16_000;
pub const MAX_SEGMENT_MS: u32 = 30_000;

pub const VAD_DIR: &str = "firered-vad";
pub const SENSE_VOICE_DIR: &str = "sensevoice-small";

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum ModelKind {
    Vad,
    Asr,
}

impl ModelKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Vad => "vad",
            Self::Asr => "asr",
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
pub struct FileSpec {
    pub path: PathBuf,
    pub sha256: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
pub struct RuntimeSpec {
    pub backend: String,
    pub format: String,
    pub precision: String,
    pub max_segment_ms: u32,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
pub struct ModelManifest {
    pub schema: u32,
    pub id: String,
    pub kind: ModelKind,
    pub version: String,
    pub license: String,
    pub files: Vec<FileSpec>,
    pub runtime: RuntimeSpec,
}

#[derive(Clone, Debug)]
pub struct ModelSet {
    pub root: PathBuf,
    pub vad: ModelManifest,
    pub asr: ModelManifest,
}

impl ModelManifest {
    pub fn load(path: &Path) -> anyhow::Result<Self> {
        let metadata = std::fs::symlink_metadata(path)
            .with_context(|| format!("无法读取模型清单 {}", path.display()))?;
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            bail!("模型清单必须是普通文件：{}", path.display());
        }
        let file =
            File::open(path).with_context(|| format!("无法读取模型清单 {}", path.display()))?;
        let manifest: Self = serde_json::from_reader(BufReader::new(file))
            .with_context(|| format!("模型清单格式错误 {}", path.display()))?;
        if path.file_name().and_then(|name| name.to_str()) == Some(MANIFEST_FILE) {
            if let Some(name) = path
                .parent()
                .and_then(|dir| dir.file_name())
                .and_then(|name| name.to_str())
            {
                if manifest.id != name {
                    bail!("模型 id {} 与目录名 {name} 不一致", manifest.id);
                }
                if name == VAD_DIR && manifest.kind != ModelKind::Vad {
                    bail!("{name} 必须是 vad 模型");
                }
                if name == SENSE_VOICE_DIR && manifest.kind != ModelKind::Asr {
                    bail!("{name} 必须是 asr 模型");
                }
            }
        }
        manifest.validate_runtime()?;
        if manifest.files.is_empty() {
            bail!("模型清单 files 不能为空：{}", path.display());
        }
        let mut seen = HashSet::new();
        for file in &manifest.files {
            if !seen.insert(file.path.clone()) {
                bail!("模型清单包含重复文件：{}", file.path.display());
            }
        }
        Ok(manifest)
    }

    pub fn verify(&self, dir: &Path) -> anyhow::Result<()> {
        if let Some(name) = dir.file_name().and_then(|name| name.to_str()) {
            if self.id != name {
                bail!("模型 id {} 与目录名 {name} 不一致", self.id);
            }
        }
        let canonical_dir = dir
            .canonicalize()
            .with_context(|| format!("无法解析模型目录 {}", dir.display()))?;
        for file in &self.files {
            let path = resolve_inside(dir, &file.path)?;
            ensure_file(&path)?;
            let canonical = path
                .canonicalize()
                .with_context(|| format!("无法解析模型文件 {}", path.display()))?;
            if !canonical.starts_with(&canonical_dir) {
                bail!("模型文件逃逸出模型目录：{}", path.display());
            }
            verify_file(&path, &file.sha256)?;
        }
        Ok(())
    }

    pub fn rebase(&mut self, dir: &Path) -> anyhow::Result<()> {
        for file in &mut self.files {
            file.path = resolve_inside(dir, &file.path)?;
        }
        Ok(())
    }

    pub fn file(&self, name: &str) -> Option<&Path> {
        self.files
            .iter()
            .map(|file| file.path.as_path())
            .find(|path| path.file_name().and_then(|value| value.to_str()) == Some(name))
    }

    fn validate_runtime(&self) -> anyhow::Result<()> {
        if self.schema != SCHEMA_VERSION {
            bail!(
                "不支持的模型清单 schema {}（期望 {SCHEMA_VERSION}）",
                self.schema
            );
        }
        if self.runtime.backend != "cpu" {
            bail!(
                "核心模型 runtime.backend 必须为 cpu，实际 {}",
                self.runtime.backend
            );
        }
        if self.runtime.format != "onnx" {
            bail!(
                "核心模型 runtime.format 必须为 onnx，实际 {}",
                self.runtime.format
            );
        }
        if self.runtime.precision.is_empty() {
            bail!("模型 runtime.precision 不能为空");
        }
        if self.kind == ModelKind::Asr && self.runtime.precision != "int8" {
            bail!(
                "SenseVoice runtime.precision 必须为 int8，实际 {}",
                self.runtime.precision
            );
        }
        if self.runtime.max_segment_ms == 0 || self.runtime.max_segment_ms > MAX_SEGMENT_MS {
            bail!(
                "模型 runtime.max_segment_ms 必须在 1..={MAX_SEGMENT_MS} 之间，实际 {}",
                self.runtime.max_segment_ms
            );
        }
        Ok(())
    }
}

pub fn load_models(root: &Path) -> anyhow::Result<ModelSet> {
    let vad_dir = root.join(VAD_DIR);
    reject_directory_symlink(&vad_dir)?;
    let mut vad = ModelManifest::load(&vad_dir.join(MANIFEST_FILE))
        .with_context(|| format!("加载 VAD 清单失败 {}", vad_dir.display()))?;
    if vad.kind != ModelKind::Vad {
        bail!("{VAD_DIR} 必须是 vad 模型，实际 {}", vad.kind.as_str());
    }
    vad.verify(&vad_dir)
        .with_context(|| format!("校验 VAD 模型失败 {}", vad_dir.display()))?;
    vad.rebase(&vad_dir)
        .with_context(|| format!("解析 VAD 模型路径失败 {}", vad_dir.display()))?;

    let asr_dir = root.join(SENSE_VOICE_DIR);
    reject_directory_symlink(&asr_dir)?;
    let mut asr = ModelManifest::load(&asr_dir.join(MANIFEST_FILE))
        .with_context(|| format!("加载 SenseVoice 清单失败 {}", asr_dir.display()))?;
    if asr.kind != ModelKind::Asr {
        bail!(
            "{SENSE_VOICE_DIR} 必须是 asr 模型，实际 {}",
            asr.kind.as_str()
        );
    }
    asr.verify(&asr_dir)
        .with_context(|| format!("校验 SenseVoice 模型失败 {}", asr_dir.display()))?;
    asr.rebase(&asr_dir)
        .with_context(|| format!("解析 SenseVoice 模型路径失败 {}", asr_dir.display()))?;

    Ok(ModelSet {
        root: root.to_path_buf(),
        vad,
        asr,
    })
}

fn reject_directory_symlink(path: &Path) -> anyhow::Result<()> {
    let metadata = std::fs::symlink_metadata(path)
        .with_context(|| format!("模型目录缺失：{}", path.display()))?;
    if metadata.file_type().is_symlink() {
        bail!("模型目录不允许符号链接：{}", path.display());
    }
    if !metadata.is_dir() {
        bail!("模型路径不是目录：{}", path.display());
    }
    Ok(())
}

fn ensure_file(path: &Path) -> anyhow::Result<()> {
    let metadata = std::fs::symlink_metadata(path)
        .with_context(|| format!("模型文件缺失：{}", path.display()))?;
    if metadata.file_type().is_symlink() {
        bail!("模型文件不允许符号链接：{}", path.display());
    }
    if !metadata.is_file() {
        bail!("模型路径不是普通文件：{}", path.display());
    }
    if metadata.len() == 0 {
        bail!("模型文件为空：{}", path.display());
    }
    reject_non_model_file(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o111 != 0 {
            bail!("模型目录不允许可执行文件：{}", path.display());
        }
    }
    Ok(())
}

fn reject_non_model_file(path: &Path) -> anyhow::Result<()> {
    let extension = path
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    if matches!(
        extension.as_str(),
        "sh" | "bash"
            | "zsh"
            | "fish"
            | "py"
            | "pyc"
            | "pl"
            | "rb"
            | "js"
            | "ps1"
            | "bat"
            | "cmd"
            | "com"
            | "exe"
            | "dll"
            | "so"
            | "dylib"
    ) {
        bail!("模型目录不允许可执行文件或脚本：{}", path.display());
    }
    Ok(())
}

fn verify_file(path: &Path, expected: &str) -> anyhow::Result<()> {
    if expected.len() != 64 || !expected.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        bail!("SHA-256 格式非法：{}", path.display());
    }
    let mut reader = BufReader::new(
        File::open(path).with_context(|| format!("无法读取模型文件 {}", path.display()))?,
    );
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let read = reader
            .read(&mut buffer)
            .with_context(|| format!("读取模型文件失败 {}", path.display()))?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    let actual = format!("{:x}", hasher.finalize());
    if !actual.eq_ignore_ascii_case(expected) {
        bail!(
            "模型文件校验失败：{}（期望 {}，实际 {}）",
            path.display(),
            expected,
            actual
        );
    }
    Ok(())
}

/// Resolves a manifest-relative path while refusing absolute paths and `..`
/// components, so a malicious model directory cannot escape its directory.
pub fn resolve_inside(dir: &Path, relative: &Path) -> anyhow::Result<PathBuf> {
    if relative.as_os_str().is_empty() {
        bail!("模型清单包含空路径");
    }
    if relative.is_absolute() {
        bail!("模型清单路径必须相对：{}", relative.display());
    }
    for component in relative.components() {
        if matches!(
            component,
            Component::ParentDir | Component::RootDir | Component::Prefix(_)
        ) {
            bail!("模型清单路径非法：{}", relative.display());
        }
    }
    Ok(dir.join(relative))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest(kind: &str, id: &str) -> ModelManifest {
        serde_json::from_value(serde_json::json!({
            "schema": 5,
            "id": id,
            "kind": kind,
            "version": "2026-01-01",
            "license": "Apache-2.0",
            "files": [{"path": "model.onnx", "sha256": "0".repeat(64)}],
            "runtime": {
                "backend": "cpu",
                "format": "onnx",
                "precision": "int8",
                "max_segment_ms": 30000
            }
        }))
        .unwrap()
    }

    #[test]
    fn accepts_schema_five() {
        let value = manifest("vad", "firered-vad");
        assert_eq!(value.schema, 5);
        value.validate_runtime().unwrap();
    }

    #[test]
    fn rejects_old_schema_and_bad_runtime() {
        let mut value = manifest("vad", "firered-vad");
        value.schema = 4;
        assert!(value.validate_runtime().is_err());
        let mut value = manifest("vad", "firered-vad");
        value.runtime.backend = "cuda".to_string();
        assert!(value.validate_runtime().is_err());
        let mut value = manifest("asr", "sensevoice-small");
        value.runtime.max_segment_ms = 60_000;
        assert!(value.validate_runtime().is_err());
    }

    #[test]
    fn rejects_parent_directory_paths() {
        let dir = Path::new("/tmp/models");
        assert!(resolve_inside(dir, Path::new("../secret")).is_err());
        assert!(resolve_inside(dir, Path::new("/etc/passwd")).is_err());
        assert!(resolve_inside(dir, Path::new("vad/model.onnx")).is_ok());
    }
}
