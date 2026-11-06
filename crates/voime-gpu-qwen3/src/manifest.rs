use std::fs::File;
use std::io::{BufReader, Read};
use std::path::{Component, Path, PathBuf};

use anyhow::{bail, Context};
use serde::Deserialize;
use sha2::{Digest, Sha256};

pub const MANIFEST_FILE: &str = "manifest.json";
pub const SCHEMA_VERSION: u32 = 5;
pub const SAMPLE_RATE: u32 = 16_000;
pub const MAX_SEGMENT_MS: u32 = 30_000;
pub const MAIN_MODEL_FILE: &str = "Qwen3-ASR-1.7B-bf16.gguf";
pub const MMPROJ_FILE: &str = "mmproj-Qwen3-ASR-1.7B-bf16.gguf";

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
pub struct Manifest {
    pub schema: u32,
    pub id: String,
    pub kind: String,
    pub version: String,
    pub license: String,
    pub files: Vec<FileSpec>,
    pub runtime: RuntimeSpec,
}

impl Manifest {
    pub fn load(dir: &Path) -> anyhow::Result<Self> {
        let metadata = std::fs::symlink_metadata(dir)
            .with_context(|| format!("无法读取模型目录 {}", dir.display()))?;
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            bail!("模型目录非法：{}", dir.display());
        }
        let path = dir.join(MANIFEST_FILE);
        let metadata = std::fs::symlink_metadata(&path)
            .with_context(|| format!("无法读取模型清单 {}", path.display()))?;
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            bail!("模型清单必须是普通文件：{}", path.display());
        }
        let file =
            File::open(&path).with_context(|| format!("无法读取模型清单 {}", path.display()))?;
        let manifest: Self = serde_json::from_reader(BufReader::new(file))
            .with_context(|| format!("模型清单格式错误 {}", path.display()))?;
        if manifest.schema != SCHEMA_VERSION {
            bail!(
                "不支持的模型清单 schema {}（期望 {SCHEMA_VERSION}）",
                manifest.schema
            );
        }
        if let Some(name) = dir.file_name().and_then(|name| name.to_str()) {
            if manifest.id != name {
                bail!("模型 id {} 与目录名 {name} 不一致", manifest.id);
            }
        }
        if manifest.kind != "asr" {
            bail!("Qwen 模型 kind 必须为 asr，实际 {}", manifest.kind);
        }
        if manifest.runtime.backend != "qwen3" {
            bail!("runtime.backend 必须为 qwen3");
        }
        if manifest.runtime.format != "gguf" {
            bail!("runtime.format 必须为 gguf");
        }
        if manifest.runtime.precision != "bf16" {
            bail!("runtime.precision 必须为 bf16");
        }
        if manifest.runtime.max_segment_ms == 0 || manifest.runtime.max_segment_ms > MAX_SEGMENT_MS
        {
            bail!("max_segment_ms 必须在 1..=30000 之间");
        }
        Ok(manifest)
    }

    pub fn verify(&self, dir: &Path) -> anyhow::Result<()> {
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
        self.main_model()?;
        self.mmproj()?;
        Ok(())
    }

    pub fn main_model(&self) -> anyhow::Result<&FileSpec> {
        self.files
            .iter()
            .find(|file| file.path.ends_with(MAIN_MODEL_FILE))
            .context("manifest 缺少 BF16 主模型")
    }

    pub fn mmproj(&self) -> anyhow::Result<&FileSpec> {
        self.files
            .iter()
            .find(|file| file.path.ends_with(MMPROJ_FILE))
            .context("manifest 缺少 BF16 mmproj")
    }

    pub fn max_segment_samples(&self) -> usize {
        (self.runtime.max_segment_ms as usize * SAMPLE_RATE as usize) / 1000
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
    let mut reader = BufReader::new(File::open(path)?);
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let read = reader.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    let actual = format!("{:x}", hasher.finalize());
    if !actual.eq_ignore_ascii_case(expected) {
        bail!("模型文件校验失败：{}", path.display());
    }
    Ok(())
}
