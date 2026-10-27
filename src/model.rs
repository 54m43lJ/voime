use std::fs::File;
use std::io::{BufReader, Read};
use std::path::{Component, Path, PathBuf};

use anyhow::{bail, Context};
use serde::Deserialize;
use sha2::{Digest, Sha256};

pub const MANIFEST_FILE: &str = "manifest.json";
pub const SCHEMA_VERSION: u32 = 1;

#[derive(Clone, Debug, Deserialize)]
pub struct TensorSpec {
    pub name: String,
    pub dtype: String,
    pub shape: Vec<i64>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct Artifact {
    pub model: PathBuf,
    pub sha256: String,
    #[serde(default)]
    pub inputs: Vec<TensorSpec>,
    #[serde(default)]
    pub outputs: Vec<TensorSpec>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct AsrArtifact {
    pub encoder: Artifact,
    pub decoder: Artifact,
    pub tokens: PathBuf,
}

#[derive(Clone, Debug, Deserialize)]
pub struct PuncArtifact {
    pub model: Artifact,
    pub vocab: PathBuf,
    pub labels: PathBuf,
    #[serde(default = "default_punc_max_tokens")]
    pub max_tokens: usize,
}

#[derive(Clone, Debug, Deserialize)]
pub struct ModelManifest {
    pub schema: u32,
    pub sample_rate: u32,
    #[serde(default)]
    pub fbank: FbankManifest,
    #[serde(default = "default_max_segment_ms")]
    pub max_segment_ms: u32,
    pub vad: Artifact,
    pub asr: AsrArtifact,
    pub punc: PuncArtifact,
}

#[derive(Clone, Debug, Deserialize)]
pub struct FbankManifest {
    #[serde(default = "default_num_bins")]
    pub num_bins: usize,
    #[serde(default = "default_frame_ms")]
    pub frame_ms: u32,
    #[serde(default = "default_shift_ms")]
    pub shift_ms: u32,
}

impl Default for FbankManifest {
    fn default() -> Self {
        Self {
            num_bins: default_num_bins(),
            frame_ms: default_frame_ms(),
            shift_ms: default_shift_ms(),
        }
    }
}

fn default_num_bins() -> usize {
    crate::asr::features::NUM_BINS
}

fn default_frame_ms() -> u32 {
    crate::asr::features::FRAME_MS
}

fn default_shift_ms() -> u32 {
    crate::asr::features::SHIFT_MS
}

fn default_max_segment_ms() -> u32 {
    10_000
}

impl ModelManifest {
    pub fn load(path: &Path) -> anyhow::Result<Self> {
        let file =
            File::open(path).with_context(|| format!("无法读取模型清单 {}", path.display()))?;
        let manifest: Self = serde_json::from_reader(BufReader::new(file))
            .with_context(|| format!("模型清单格式错误 {}", path.display()))?;
        if manifest.schema != SCHEMA_VERSION {
            bail!(
                "不支持的模型清单版本 {}（期望 {}）",
                manifest.schema,
                SCHEMA_VERSION
            );
        }
        if manifest.sample_rate != crate::audio::resample::TARGET_SAMPLE_RATE {
            bail!(
                "模型采样率 {} 与内部采样率 {} 不一致",
                manifest.sample_rate,
                crate::audio::resample::TARGET_SAMPLE_RATE
            );
        }
        manifest.validate_specs()?;
        Ok(manifest)
    }

    /// Checks that every declared model/asset exists and that every ONNX model
    /// still matches its SHA-256 digest.
    pub fn verify(&self, dir: &Path) -> anyhow::Result<()> {
        for artifact in self.artifacts() {
            let path = resolve_inside(dir, &artifact.model)?;
            verify_file(&path, &artifact.sha256)?;
        }
        for aux in [&self.asr.tokens, &self.punc.vocab, &self.punc.labels] {
            let path = resolve_inside(dir, aux)?;
            ensure_file(&path)?;
        }
        Ok(())
    }

    /// Converts all manifest-relative paths into paths under `dir`. Call after
    /// `verify`, because `verify` deliberately rejects absolute paths.
    pub fn rebase(&mut self, dir: &Path) -> anyhow::Result<()> {
        for artifact in [
            &mut self.vad,
            &mut self.asr.encoder,
            &mut self.asr.decoder,
            &mut self.punc.model,
        ] {
            artifact.model = resolve_inside(dir, &artifact.model)?;
        }
        self.asr.tokens = resolve_inside(dir, &self.asr.tokens)?;
        self.punc.vocab = resolve_inside(dir, &self.punc.vocab)?;
        self.punc.labels = resolve_inside(dir, &self.punc.labels)?;
        Ok(())
    }

    fn artifacts(&self) -> [&Artifact; 4] {
        [
            &self.vad,
            &self.asr.encoder,
            &self.asr.decoder,
            &self.punc.model,
        ]
    }

    fn validate_specs(&self) -> anyhow::Result<()> {
        for (label, artifact) in [
            ("vad", &self.vad),
            ("asr.encoder", &self.asr.encoder),
            ("asr.decoder", &self.asr.decoder),
            ("punc", &self.punc.model),
        ] {
            if artifact.inputs.is_empty() {
                bail!("{label} 未声明任何输入张量");
            }
            if artifact.outputs.is_empty() {
                bail!("{label} 未声明任何输出张量");
            }
            let mut names = std::collections::HashSet::new();
            for spec in artifact.inputs.iter().chain(artifact.outputs.iter()) {
                if !names.insert(&spec.name) {
                    bail!("{label} 张量名重复：{}", spec.name);
                }
            }
        }
        if self.punc.max_tokens == 0 {
            bail!("punc.max_tokens 必须大于 0");
        }
        if self.fbank.num_bins == 0 || self.fbank.frame_ms == 0 || self.fbank.shift_ms == 0 {
            bail!("fbank 参数必须大于 0");
        }
        if self.max_segment_ms == 0 || self.max_segment_ms > 60_000 {
            bail!("max_segment_ms 必须在 1..=60000 之间");
        }
        Ok(())
    }
}

fn default_punc_max_tokens() -> usize {
    400
}

fn ensure_file(path: &Path) -> anyhow::Result<()> {
    let metadata =
        std::fs::metadata(path).with_context(|| format!("模型文件缺失：{}", path.display()))?;
    if !metadata.is_file() {
        bail!("模型路径不是普通文件：{}", path.display());
    }
    if metadata.len() == 0 {
        bail!("模型文件为空：{}", path.display());
    }
    Ok(())
}

fn verify_file(path: &Path, expected: &str) -> anyhow::Result<()> {
    ensure_file(path)?;
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
/// components, so a malicious model package cannot escape its directory.
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

    #[test]
    fn rejects_parent_directory_paths() {
        let dir = Path::new("/tmp/models");
        assert!(resolve_inside(dir, Path::new("../secret")).is_err());
        assert!(resolve_inside(dir, Path::new("/etc/passwd")).is_err());
        assert!(resolve_inside(dir, Path::new("vad/model.onnx")).is_ok());
    }
}
