use std::collections::HashSet;
use std::fs::File;
use std::io::{BufReader, Read};
use std::path::{Component, Path, PathBuf};

use anyhow::{bail, Context};
use serde::Deserialize;
use sha2::{Digest, Sha256};

pub const MANIFEST_FILE: &str = "manifest.json";
pub const SCHEMA_VERSION: u32 = 3;
pub const SAMPLES_PER_SECOND: u32 = 16_000;

/// Resource pack directories below the model root. A pack owns one directory
/// and the model directories below it; installers and the loader share the
/// names.
pub const DEFAULT_PACK_DIR: &str = "default";
pub const FIRERED_PACK_DIR: &str = "firered";
pub const VAD_DIR: &str = "vad";
pub const SENSE_VOICE_DIR: &str = "sense-voice";
pub const FIRE_RED_AED_DIR: &str = "fire-red-aed";
pub const PUNC_DIR: &str = "punc";

pub fn default_pack_root(model_root: &Path) -> PathBuf {
    model_root.join(DEFAULT_PACK_DIR)
}

pub fn firered_pack_root(model_root: &Path) -> PathBuf {
    model_root.join(FIRERED_PACK_DIR)
}

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

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum Precision {
    Fp16,
    /// Temporary compatibility for older official INT8 packages.
    Int8,
}

impl Precision {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Fp16 => "fp16",
            Self::Int8 => "int8",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PuncPolicy {
    Builtin,
    External,
}

#[derive(Clone, Debug, Deserialize)]
pub struct FbankManifest {
    pub num_bins: u32,
    pub frame_ms: u32,
    pub shift_ms: u32,
    pub window_type: String,
}

#[derive(Clone, Debug, Deserialize)]
pub struct AsrOptions {
    #[serde(default)]
    pub language: Option<String>,
    #[serde(default)]
    pub use_itn: bool,
    #[serde(default = "default_partial")]
    pub partial: bool,
}

impl Default for AsrOptions {
    fn default() -> Self {
        Self {
            language: None,
            use_itn: false,
            partial: true,
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
pub struct VadManifest {
    pub schema: u32,
    pub kind: String,
    pub fbank: FbankManifest,
    pub model: Artifact,
}

#[derive(Clone, Debug, Deserialize)]
pub struct SenseVoiceManifest {
    pub schema: u32,
    pub kind: String,
    pub fbank: FbankManifest,
    pub max_segment_ms: u32,
    pub model: Artifact,
    pub tokens: PathBuf,
    pub token_count: usize,
    #[serde(default)]
    pub options: AsrOptions,
}

#[derive(Clone, Debug, Deserialize)]
pub struct FireRedManifest {
    pub schema: u32,
    pub kind: String,
    pub fbank: FbankManifest,
    pub max_segment_ms: u32,
    pub encoder: Artifact,
    pub decoder: Artifact,
    pub precision: Precision,
    pub tokens: PathBuf,
    pub token_count: usize,
}

#[derive(Clone, Debug, Deserialize)]
pub struct PuncManifest {
    pub schema: u32,
    pub kind: String,
    pub model: Artifact,
    pub vocab: PathBuf,
    pub labels: PathBuf,
    pub max_tokens: usize,
}

fn default_partial() -> bool {
    true
}

impl VadManifest {
    pub fn load(path: &Path) -> anyhow::Result<Self> {
        let manifest: Self = load_json(path)?;
        manifest.validate()?;
        Ok(manifest)
    }

    pub fn validate(&self) -> anyhow::Result<()> {
        validate_common(self.schema, &self.kind, "vad")?;
        validate_fbank(&self.fbank)?;
        validate_artifact(&self.model, "vad")?;
        Ok(())
    }

    pub fn verify(&self, dir: &Path) -> anyhow::Result<()> {
        verify_artifact(dir, &self.model, "vad")
    }

    pub fn rebase(&mut self, dir: &Path) -> anyhow::Result<()> {
        rebase_artifact(dir, &mut self.model)
    }
}

impl SenseVoiceManifest {
    pub fn load(path: &Path) -> anyhow::Result<Self> {
        let manifest: Self = load_json(path)?;
        manifest.validate()?;
        Ok(manifest)
    }

    pub fn validate(&self) -> anyhow::Result<()> {
        validate_common(self.schema, &self.kind, "sense-voice")?;
        validate_fbank(&self.fbank)?;
        validate_artifact(&self.model, "sense-voice")?;
        if self.token_count != 25_055 {
            bail!(
                "sense-voice token_count 必须为 25055（实际 {}）",
                self.token_count
            );
        }
        if self.max_segment_ms == 0 || self.max_segment_ms > 60_000 {
            bail!("sense-voice max_segment_ms 必须在 1..=60000 之间");
        }
        if let Some(language) = self.options.language.as_deref() {
            if !matches!(language, "auto" | "zh" | "en" | "ja" | "ko" | "yue") {
                bail!("SenseVoice 不支持语言：{language}");
            }
        }
        if !self.options.use_itn {
            bail!("SenseVoice 内置标点要求 use_itn = true");
        }
        Ok(())
    }

    pub fn verify(&self, dir: &Path) -> anyhow::Result<()> {
        verify_artifact(dir, &self.model, "sense-voice")?;
        ensure_file(&resolve_inside(dir, &self.tokens)?)
    }

    pub fn rebase(&mut self, dir: &Path) -> anyhow::Result<()> {
        rebase_artifact(dir, &mut self.model)?;
        self.tokens = resolve_inside(dir, &self.tokens)?;
        Ok(())
    }
}

impl FireRedManifest {
    pub fn load(path: &Path) -> anyhow::Result<Self> {
        let manifest: Self = load_json(path)?;
        manifest.validate()?;
        Ok(manifest)
    }

    pub fn validate(&self) -> anyhow::Result<()> {
        validate_common(self.schema, &self.kind, "fire-red-aed")?;
        validate_fbank(&self.fbank)?;
        validate_artifact(&self.encoder, "fire-red-aed.encoder")?;
        validate_artifact(&self.decoder, "fire-red-aed.decoder")?;
        if self.max_segment_ms == 0 || self.max_segment_ms > 60_000 {
            bail!("fire-red-aed max_segment_ms 必须在 1..=60000 之间");
        }
        match self.precision {
            Precision::Fp16 => {
                if !matches!(self.token_count, 7_832 | 8_667) {
                    bail!(
                        "fire-red-aed FP16 token_count 必须为 8667（v2）或 7832（v1），实际 {}",
                        self.token_count
                    );
                }
            }
            Precision::Int8 => {
                if self.token_count != 7_832 {
                    bail!(
                        "fire-red-aed INT8 token_count 必须为 7832（实际 {}）",
                        self.token_count
                    );
                }
            }
        }
        Ok(())
    }

    pub fn verify(&self, dir: &Path) -> anyhow::Result<()> {
        verify_artifact(dir, &self.encoder, "fire-red-aed.encoder")?;
        verify_artifact(dir, &self.decoder, "fire-red-aed.decoder")?;
        ensure_file(&resolve_inside(dir, &self.tokens)?)
    }

    pub fn rebase(&mut self, dir: &Path) -> anyhow::Result<()> {
        rebase_artifact(dir, &mut self.encoder)?;
        rebase_artifact(dir, &mut self.decoder)?;
        self.tokens = resolve_inside(dir, &self.tokens)?;
        Ok(())
    }
}

impl PuncManifest {
    pub fn load(path: &Path) -> anyhow::Result<Self> {
        let manifest: Self = load_json(path)?;
        manifest.validate()?;
        Ok(manifest)
    }

    pub fn validate(&self) -> anyhow::Result<()> {
        validate_common(self.schema, &self.kind, "punc")?;
        validate_artifact(&self.model, "punc")?;
        if self.max_tokens == 0 {
            bail!("punc max_tokens 必须大于 0");
        }
        Ok(())
    }

    pub fn verify(&self, dir: &Path) -> anyhow::Result<()> {
        verify_artifact(dir, &self.model, "punc")?;
        for aux in [&self.vocab, &self.labels] {
            ensure_file(&resolve_inside(dir, aux)?)?;
        }
        Ok(())
    }

    pub fn rebase(&mut self, dir: &Path) -> anyhow::Result<()> {
        rebase_artifact(dir, &mut self.model)?;
        self.vocab = resolve_inside(dir, &self.vocab)?;
        self.labels = resolve_inside(dir, &self.labels)?;
        Ok(())
    }
}

fn load_json<T: for<'de> Deserialize<'de>>(path: &Path) -> anyhow::Result<T> {
    let file = File::open(path).with_context(|| format!("无法读取模型清单 {}", path.display()))?;
    serde_json::from_reader(BufReader::new(file))
        .with_context(|| format!("模型清单格式错误 {}", path.display()))
}

fn validate_common(schema: u32, kind: &str, expected: &str) -> anyhow::Result<()> {
    if schema != SCHEMA_VERSION {
        bail!("不支持的模型清单版本 {schema}（期望 {SCHEMA_VERSION}）");
    }
    if kind != expected {
        bail!("模型清单 kind 必须是 {expected}，实际 {kind}");
    }
    Ok(())
}

fn validate_fbank(fbank: &FbankManifest) -> anyhow::Result<()> {
    if fbank.num_bins == 0 || fbank.frame_ms == 0 || fbank.shift_ms == 0 {
        bail!("fbank 参数必须大于 0");
    }
    if !matches!(fbank.window_type.as_str(), "povey" | "hamming") {
        bail!("不支持的 fbank.window_type：{}", fbank.window_type);
    }
    Ok(())
}

fn validate_artifact(artifact: &Artifact, label: &str) -> anyhow::Result<()> {
    if artifact.inputs.is_empty() {
        bail!("{label} 未声明任何输入张量");
    }
    if artifact.outputs.is_empty() {
        bail!("{label} 未声明任何输出张量");
    }
    let mut names = HashSet::new();
    for spec in artifact.inputs.iter().chain(artifact.outputs.iter()) {
        if !names.insert(&spec.name) {
            bail!("{label} 张量名重复：{}", spec.name);
        }
    }
    Ok(())
}

fn rebase_artifact(dir: &Path, artifact: &mut Artifact) -> anyhow::Result<()> {
    artifact.model = resolve_inside(dir, &artifact.model)?;
    Ok(())
}

fn verify_artifact(dir: &Path, artifact: &Artifact, label: &str) -> anyhow::Result<()> {
    let path = resolve_inside(dir, &artifact.model)?;
    verify_file(&path, &artifact.sha256)
        .with_context(|| format!("{label} 校验失败 {}", path.display()))
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

    fn artifact(name: &str) -> serde_json::Value {
        serde_json::json!({
            "model": name,
            "sha256": "0".repeat(64),
            "inputs": [{"name": "x", "dtype": "float32", "shape": [1, -1, 80]}],
            "outputs": [{"name": "y", "dtype": "float32", "shape": [1, -1, 1]}]
        })
    }

    #[test]
    fn validates_each_manifest_kind() {
        let vad: VadManifest = serde_json::from_value(serde_json::json!({
            "schema": 3, "kind": "vad",
            "fbank": {"num_bins": 80, "frame_ms": 25, "shift_ms": 10, "window_type": "hamming"},
            "model": artifact("stream-vad.onnx")
        }))
        .unwrap();
        vad.validate().unwrap();

        let sense: SenseVoiceManifest = serde_json::from_value(serde_json::json!({
            "schema": 3, "kind": "sense-voice",
            "fbank": {"num_bins": 80, "frame_ms": 25, "shift_ms": 10, "window_type": "hamming"},
            "max_segment_ms": 20000,
            "model": artifact("model.int8.onnx"),
            "tokens": "tokens.txt", "token_count": 25055,
            "options": {"use_itn": true}
        }))
        .unwrap();
        sense.validate().unwrap();

        let red: FireRedManifest = serde_json::from_value(serde_json::json!({
            "schema": 3, "kind": "fire-red-aed",
            "fbank": {"num_bins": 80, "frame_ms": 25, "shift_ms": 10, "window_type": "povey"},
            "max_segment_ms": 30000,
            "encoder": artifact("encoder.fp16.onnx"),
            "decoder": artifact("decoder.fp16.onnx"),
            "precision": "fp16",
            "tokens": "tokens.txt", "token_count": 7832
        }))
        .unwrap();
        red.validate().unwrap();

        let punc: PuncManifest = serde_json::from_value(serde_json::json!({
            "schema": 3, "kind": "punc",
            "model": artifact("model.onnx"),
            "vocab": "vocab.txt", "labels": "labels.json", "max_tokens": 400
        }))
        .unwrap();
        punc.validate().unwrap();
    }

    #[test]
    fn rejects_wrong_kind_and_old_schema() {
        let value = serde_json::json!({
            "schema": 3, "kind": "punc",
            "fbank": {"num_bins": 80, "frame_ms": 25, "shift_ms": 10, "window_type": "hamming"},
            "model": artifact("stream-vad.onnx")
        });
        let vad: VadManifest = serde_json::from_value(value).unwrap();
        assert!(vad.validate().is_err());

        let value = serde_json::json!({
            "schema": 2, "kind": "vad",
            "fbank": {"num_bins": 80, "frame_ms": 25, "shift_ms": 10, "window_type": "hamming"},
            "model": artifact("stream-vad.onnx")
        });
        let vad: VadManifest = serde_json::from_value(value).unwrap();
        assert!(vad.validate().is_err());
    }

    #[test]
    fn rejects_parent_directory_paths() {
        let dir = Path::new("/tmp/models");
        assert!(resolve_inside(dir, Path::new("../secret")).is_err());
        assert!(resolve_inside(dir, Path::new("/etc/passwd")).is_err());
        assert!(resolve_inside(dir, Path::new("vad/stream-vad.onnx")).is_ok());
    }
}
