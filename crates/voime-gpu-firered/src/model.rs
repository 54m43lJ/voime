use std::collections::HashSet;
use std::fs::File;
use std::io::{BufReader, Read};
use std::path::{Component, Path, PathBuf};

use anyhow::{bail, Context};
use serde::Deserialize;
use sha2::{Digest, Sha256};

pub const MANIFEST_FILE: &str = "manifest.json";
pub const SCHEMA_VERSION: u32 = 5;
pub const ASR_DIR: &str = "fireredasr2-aed";
pub const PUNC_DIR: &str = "fireredpunc";

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum ModelKind {
    Asr,
    Punc,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
pub struct FileSpec {
    pub path: PathBuf,
    pub sha256: String,
    pub size: u64,
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
    pub asr: ModelManifest,
    pub punc: ModelManifest,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TensorSpec {
    pub name: String,
    pub dtype: String,
    pub shape: Vec<i64>,
}

pub fn load(root: &Path) -> anyhow::Result<ModelSet> {
    let asr = load_one(&root.join(ASR_DIR), ModelKind::Asr)?;
    let punc = load_one(&root.join(PUNC_DIR), ModelKind::Punc)?;
    Ok(ModelSet { asr, punc })
}

fn load_one(dir: &Path, kind: ModelKind) -> anyhow::Result<ModelManifest> {
    let metadata = std::fs::symlink_metadata(dir)
        .with_context(|| format!("无法读取模型目录 {}", dir.display()))?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        bail!("模型目录非法：{}", dir.display());
    }
    let mut manifest = ModelManifest::load(&dir.join(MANIFEST_FILE))
        .with_context(|| format!("加载模型清单失败 {}", dir.display()))?;
    if manifest.kind != kind {
        bail!("{} 的 kind 不匹配", dir.display());
    }
    if manifest.runtime.backend != "firered" {
        bail!("{} 的 runtime.backend 必须为 firered", dir.display());
    }
    if manifest.runtime.format != "onnx" {
        bail!("{} 的 runtime.format 必须为 onnx", dir.display());
    }
    if manifest.runtime.precision != "fp32" {
        bail!("{} 的 runtime.precision 必须为 fp32", dir.display());
    }
    if manifest.runtime.max_segment_ms == 0 || manifest.runtime.max_segment_ms > 30_000 {
        bail!("{} 的 max_segment_ms 必须在 1..=30000 之间", dir.display());
    }
    manifest
        .verify(dir)
        .with_context(|| format!("校验模型失败 {}", dir.display()))?;
    let required: &[&str] = match kind {
        ModelKind::Asr => &[
            "encoder.onnx",
            "encoder.data",
            "ctc.onnx",
            "decoder.onnx",
            "decoder.data",
            "dict.txt",
            "train_bpe1000.model",
            "cmvn.ark",
            "config.json",
        ],
        ModelKind::Punc => &["model.onnx", "tokens.txt", "out_dict", "punc.json"],
    };
    for name in required {
        manifest
            .file(name)
            .with_context(|| format!("{} 缺少必需文件 {name}", dir.display()))?;
    }
    manifest
        .rebase(dir)
        .with_context(|| format!("解析模型路径失败 {}", dir.display()))?;
    Ok(manifest)
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
        if manifest.schema != SCHEMA_VERSION {
            bail!(
                "不支持的模型清单 schema {}（期望 {SCHEMA_VERSION}）",
                manifest.schema
            );
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
            let metadata = std::fs::symlink_metadata(&path)
                .with_context(|| format!("模型文件缺失：{}", path.display()))?;
            if metadata.file_type().is_symlink() || !metadata.is_file() || metadata.len() == 0 {
                bail!("模型文件非法：{}", path.display());
            }
            if metadata.len() != file.size {
                bail!(
                    "模型文件大小不符：{}（期望 {}，实际 {}）",
                    path.display(),
                    file.size,
                    metadata.len()
                );
            }
            if !path
                .canonicalize()
                .with_context(|| format!("无法解析模型文件 {}", path.display()))?
                .starts_with(&canonical_dir)
            {
                bail!("模型文件逃逸出模型目录：{}", path.display());
            }
            reject_non_model_file(&path)?;
            verify_sha256(&path, &file.sha256)?;
        }
        Ok(())
    }

    pub fn rebase(&mut self, dir: &Path) -> anyhow::Result<()> {
        for file in &mut self.files {
            file.path = resolve_inside(dir, &file.path)?;
        }
        Ok(())
    }

    pub fn file(&self, name: &str) -> anyhow::Result<&Path> {
        self.files
            .iter()
            .map(|file| file.path.as_path())
            .find(|path| path.file_name().and_then(|value| value.to_str()) == Some(name))
            .with_context(|| format!("{} 清单缺少 {name}", self.id))
    }
}

pub fn resolve_inside(dir: &Path, relative: &Path) -> anyhow::Result<PathBuf> {
    if relative.as_os_str().is_empty() || relative.is_absolute() {
        bail!("模型清单路径非法：{}", relative.display());
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

fn verify_sha256(path: &Path, expected: &str) -> anyhow::Result<()> {
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
