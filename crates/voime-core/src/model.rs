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
/// The fixed base package that produces Preview and CPU Final.
pub const PREVIEW_PACKAGE_ID: &str = "sense-voice-2024-int8";

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

pub type VadArtifact = Artifact;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub enum AsrKind {
    SenseVoice,
    FireRedAed,
    External,
}

impl AsrKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::SenseVoice => "sense-voice",
            Self::FireRedAed => "fire-red-aed",
            Self::External => "external",
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum PuncPolicy {
    Builtin,
    External,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum Precision {
    Fp16,
}

impl Precision {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Fp16 => "fp16",
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum BackendKind {
    Builtin,
    Worker,
}

impl BackendKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Builtin => "builtin",
            Self::Worker => "worker",
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
pub struct AsrOptions {
    #[serde(default)]
    pub language: Option<String>,
    #[serde(default)]
    pub use_itn: bool,
    pub punc: PuncPolicy,
    #[serde(default = "default_partial")]
    pub partial: bool,
}

impl Default for AsrOptions {
    fn default() -> Self {
        Self {
            language: None,
            use_itn: false,
            punc: PuncPolicy::Builtin,
            partial: default_partial(),
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
pub struct AsrArtifact {
    pub kind: AsrKind,
    #[serde(default)]
    pub model: Option<Artifact>,
    #[serde(default)]
    pub encoder: Option<Artifact>,
    #[serde(default)]
    pub decoder: Option<Artifact>,
    #[serde(default)]
    pub precision: Option<Precision>,
    pub tokens: PathBuf,
    pub token_count: usize,
    #[serde(default)]
    pub options: AsrOptions,
}

impl AsrArtifact {
    pub fn artifacts(&self) -> Vec<&Artifact> {
        let mut artifacts = Vec::with_capacity(3);
        if let Some(model) = self.model.as_ref() {
            artifacts.push(model);
        }
        if let Some(encoder) = self.encoder.as_ref() {
            artifacts.push(encoder);
        }
        if let Some(decoder) = self.decoder.as_ref() {
            artifacts.push(decoder);
        }
        artifacts
    }
}

#[derive(Clone, Debug, Deserialize)]
pub struct PuncArtifact {
    pub model: Artifact,
    pub vocab: PathBuf,
    pub labels: PathBuf,
    pub max_tokens: usize,
}

#[derive(Clone, Debug, Deserialize)]
pub struct BackendManifest {
    pub kind: BackendKind,
    #[serde(default)]
    pub runtime: Option<String>,
    #[serde(default)]
    pub device: Option<String>,
    #[serde(default)]
    pub min_vram_mb: Option<u32>,
    #[serde(default = "default_supports_partial")]
    pub supports_partial: bool,
    #[serde(default = "default_load_timeout_ms")]
    pub load_timeout_ms: u64,
    #[serde(default = "default_request_timeout_ms")]
    pub request_timeout_ms: u64,
}

#[derive(Clone, Debug, Deserialize)]
pub struct ModelManifest {
    pub schema: u32,
    pub display_name: String,
    pub sample_rate: u32,
    pub fbank: FbankManifest,
    pub max_segment_ms: u32,
    pub vad: VadArtifact,
    #[serde(default)]
    pub requires: Vec<String>,
    pub backend: BackendManifest,
    pub asr: AsrArtifact,
    #[serde(default)]
    pub punc: Option<PuncArtifact>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct FbankManifest {
    pub num_bins: u32,
    pub frame_ms: u32,
    pub shift_ms: u32,
    pub window_type: String,
}

fn default_partial() -> bool {
    true
}

fn default_supports_partial() -> bool {
    true
}

fn default_load_timeout_ms() -> u64 {
    30_000
}

fn default_request_timeout_ms() -> u64 {
    30_000
}

impl ModelManifest {
    pub fn load(path: &Path) -> anyhow::Result<Self> {
        let file =
            File::open(path).with_context(|| format!("无法读取模型清单 {}", path.display()))?;
        let manifest: Self = serde_json::from_reader(BufReader::new(file))
            .with_context(|| format!("模型清单格式错误 {}", path.display()))?;
        manifest.validate()?;
        Ok(manifest)
    }

    /// Checks that every declared model/asset exists and that every ONNX model
    /// still matches its SHA-256 digest.
    pub fn verify(&self, dir: &Path) -> anyhow::Result<()> {
        for artifact in self.artifacts() {
            let path = resolve_inside(dir, &artifact.model)?;
            verify_file(&path, &artifact.sha256)?;
        }
        for aux in self.auxiliary_files() {
            let path = resolve_inside(dir, aux)?;
            ensure_file(&path)?;
        }
        Ok(())
    }

    /// Converts manifest-relative paths into paths under `dir`.
    pub fn rebase(&mut self, dir: &Path) -> anyhow::Result<()> {
        self.vad.model = resolve_inside(dir, &self.vad.model)?;
        for artifact in [
            self.asr.model.as_mut(),
            self.asr.encoder.as_mut(),
            self.asr.decoder.as_mut(),
        ]
        .into_iter()
        .flatten()
        {
            artifact.model = resolve_inside(dir, &artifact.model)?;
        }
        self.asr.tokens = resolve_inside(dir, &self.asr.tokens)?;
        if let Some(punc) = self.punc.as_mut() {
            punc.model.model = resolve_inside(dir, &punc.model.model)?;
            punc.vocab = resolve_inside(dir, &punc.vocab)?;
            punc.labels = resolve_inside(dir, &punc.labels)?;
        }
        Ok(())
    }

    pub fn validate(&self) -> anyhow::Result<()> {
        if self.schema != SCHEMA_VERSION {
            bail!(
                "不支持的模型清单版本 {}（期望 {}）",
                self.schema,
                SCHEMA_VERSION
            );
        }
        if self.sample_rate != SAMPLES_PER_SECOND {
            bail!(
                "模型采样率 {} 与内部采样率 {} 不一致",
                self.sample_rate,
                SAMPLES_PER_SECOND
            );
        }
        if self.display_name.trim().is_empty() {
            bail!("display_name 不能为空");
        }
        if self.display_name.chars().any(char::is_control) {
            bail!("display_name 不能包含控制字符");
        }
        if self.fbank.num_bins == 0 || self.fbank.frame_ms == 0 || self.fbank.shift_ms == 0 {
            bail!("fbank 参数必须大于 0");
        }
        if !matches!(self.fbank.window_type.as_str(), "povey" | "hamming") {
            bail!("不支持的 fbank.window_type：{}", self.fbank.window_type);
        }
        if self.max_segment_ms == 0 || self.max_segment_ms > 60_000 {
            bail!("max_segment_ms 必须在 1..=60000 之间");
        }
        if self.asr.token_count == 0 {
            bail!("asr.token_count 必须大于 0");
        }
        if self.backend.load_timeout_ms == 0 || self.backend.request_timeout_ms == 0 {
            bail!("backend 超时必须大于 0");
        }
        self.validate_requires()?;
        self.validate_kind()?;
        self.validate_punc()?;
        self.validate_specs()?;
        Ok(())
    }

    fn validate_requires(&self) -> anyhow::Result<()> {
        match self.backend.kind {
            BackendKind::Builtin => {
                if !self.requires.is_empty() {
                    bail!("builtin 包不应声明 requires");
                }
            }
            BackendKind::Worker => {
                if !self.requires.iter().any(|id| id == PREVIEW_PACKAGE_ID) {
                    bail!("worker 包必须声明 requires: [\"{PREVIEW_PACKAGE_ID}\"]");
                }
                if self.backend.runtime.as_deref().unwrap_or("").is_empty() {
                    bail!("worker 包必须声明 backend.runtime");
                }
                if self.backend.device.as_deref().unwrap_or("").is_empty() {
                    bail!("worker 包必须声明 backend.device");
                }
                if self.backend.min_vram_mb.unwrap_or(0) == 0 {
                    bail!("worker 包必须声明 backend.min_vram_mb");
                }
                if self.backend.supports_partial {
                    bail!("worker Final 不支持 backend.supports_partial = true");
                }
            }
        }
        Ok(())
    }

    fn validate_kind(&self) -> anyhow::Result<()> {
        let options = &self.asr.options;
        match self.asr.kind {
            AsrKind::SenseVoice => {
                if self.asr.model.is_none() {
                    bail!("sense-voice 包必须声明 asr.model");
                }
                if self.asr.encoder.is_some() || self.asr.decoder.is_some() {
                    bail!("sense-voice 包不能声明 asr.encoder / asr.decoder");
                }
                if self.asr.precision.is_some() {
                    bail!("sense-voice 包不能声明 asr.precision");
                }
                if options.punc != PuncPolicy::Builtin {
                    bail!("sense-voice 包必须使用 punc = builtin");
                }
                if !options.use_itn {
                    bail!("sense-voice builtin 标点要求 use_itn = true");
                }
                if options.partial && !self.backend.supports_partial {
                    bail!("sense-voice 包声明 partial 但 backend 不支持");
                }
                if self.asr.token_count != 25_055 {
                    bail!(
                        "sense-voice token_count 必须为 25055（实际 {}）",
                        self.asr.token_count
                    );
                }
                if let Some(language) = options.language.as_deref() {
                    if !matches!(language, "auto" | "zh" | "en" | "ja" | "ko" | "yue") {
                        bail!("SenseVoice 不支持语言：{language}");
                    }
                }
                if self.backend.kind != BackendKind::Builtin {
                    bail!("sense-voice 包必须使用 backend.kind = builtin");
                }
            }
            AsrKind::FireRedAed => {
                if self.asr.model.is_some() {
                    bail!("fire-red-aed 包不能声明 asr.model");
                }
                if self.asr.encoder.is_none() || self.asr.decoder.is_none() {
                    bail!("fire-red-aed 包必须同时声明 asr.encoder 与 asr.decoder");
                }
                if self.asr.precision != Some(Precision::Fp16) {
                    bail!("fire-red-aed 包必须声明 precision = fp16");
                }
                if options.punc != PuncPolicy::External {
                    bail!("fire-red-aed 包必须使用 punc = external");
                }
                if options.language.is_some() {
                    bail!("fire-red-aed 包不支持 asr.options.language");
                }
                if self.backend.kind != BackendKind::Worker {
                    bail!("fire-red-aed 包必须使用 backend.kind = worker");
                }
                if options.partial {
                    bail!("fire-red-aed 包不能请求 partial");
                }
                if self.asr.token_count != 8_667 {
                    bail!(
                        "fire-red-aed token_count 必须为 8667（实际 {}）",
                        self.asr.token_count
                    );
                }
            }
            AsrKind::External => {
                bail!("external ASR 尚未定义模型包契约");
            }
        }
        Ok(())
    }

    fn validate_punc(&self) -> anyhow::Result<()> {
        match self.asr.options.punc {
            PuncPolicy::Builtin if self.punc.is_some() => {
                bail!("builtin 标点包不应携带 punc 段");
            }
            PuncPolicy::External if self.punc.is_none() => {
                bail!("external 标点需要顶层 punc 段");
            }
            _ => {}
        }
        if let Some(punc) = self.punc.as_ref() {
            if punc.max_tokens == 0 {
                bail!("punc.max_tokens 必须大于 0");
            }
        }
        Ok(())
    }

    fn validate_specs(&self) -> anyhow::Result<()> {
        for artifact in self.artifacts() {
            let label = artifact.model.display().to_string();
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
        }
        Ok(())
    }

    fn artifacts(&self) -> Vec<&Artifact> {
        let mut artifacts = vec![&self.vad];
        artifacts.extend(self.asr.artifacts());
        if let Some(punc) = self.punc.as_ref() {
            artifacts.push(&punc.model);
        }
        artifacts
    }

    fn auxiliary_files(&self) -> Vec<&PathBuf> {
        let mut files = vec![&self.asr.tokens];
        if let Some(punc) = self.punc.as_ref() {
            files.push(&punc.vocab);
            files.push(&punc.labels);
        }
        files
    }
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

    fn sense_voice_manifest() -> serde_json::Value {
        serde_json::json!({
            "schema": 3,
            "display_name": "SenseVoice 2024 ITN",
            "sample_rate": 16000,
            "fbank": {"num_bins": 80, "frame_ms": 25, "shift_ms": 10, "window_type": "hamming"},
            "max_segment_ms": 20000,
            "vad": {
                "model": "vad/stream-vad.onnx",
                "sha256": "0".repeat(64),
                "inputs": [{"name": "feat", "dtype": "float32", "shape": [1, -1, 80]}],
                "outputs": [{"name": "probs", "dtype": "float32", "shape": [1, -1, 1]}]
            },
            "backend": {"kind": "builtin", "supports_partial": true,
                        "load_timeout_ms": 30000, "request_timeout_ms": 30000},
            "asr": {
                "kind": "sense-voice",
                "model": {
                    "model": "asr/model.int8.onnx",
                    "sha256": "0".repeat(64),
                    "inputs": [{"name": "x", "dtype": "float32", "shape": [1, -1, 560]}],
                    "outputs": [{"name": "logits", "dtype": "float32", "shape": [1, -1, 25055]}]
                },
                "tokens": "asr/tokens.txt",
                "token_count": 25055,
                "options": {"language": "auto", "use_itn": true, "punc": "builtin", "partial": true}
            }
        })
    }

    #[test]
    fn accepts_documented_sense_voice_package() {
        let manifest: ModelManifest =
            serde_json::from_value(sense_voice_manifest()).expect("parse");
        manifest.validate().unwrap();
    }

    #[test]
    fn rejects_old_schema() {
        let mut value = sense_voice_manifest();
        value["schema"] = serde_json::json!(2);
        let manifest: ModelManifest = serde_json::from_value(value).unwrap();
        assert!(manifest.validate().is_err());
    }

    #[test]
    fn rejects_builtin_package_with_punc() {
        let mut value = sense_voice_manifest();
        value["punc"] = serde_json::json!({
            "model": {"model": "punc/model.onnx", "sha256": "0".repeat(64),
                       "inputs": [{"name": "input_ids", "dtype": "int64", "shape": [1, -1]}],
                       "outputs": [{"name": "logits", "dtype": "float32", "shape": [1, -1, -1]}]},
            "vocab": "punc/vocab.txt",
            "labels": "punc/labels.json",
            "max_tokens": 400
        });
        let manifest: ModelManifest = serde_json::from_value(value).unwrap();
        assert!(manifest.validate().is_err());
    }

    #[test]
    fn rejects_parent_directory_paths() {
        let dir = Path::new("/tmp/models");
        assert!(resolve_inside(dir, Path::new("../secret")).is_err());
        assert!(resolve_inside(dir, Path::new("/etc/passwd")).is_err());
        assert!(resolve_inside(dir, Path::new("vad/model.onnx")).is_ok());
    }

    fn aed_manifest(runtime: &str) -> serde_json::Value {
        serde_json::json!({
            "schema": 3,
            "display_name": "FireRedASR2-AED (GPU, FP16)",
            "requires": ["sense-voice-2024-int8"],
            "sample_rate": 16000,
            "fbank": {"num_bins": 80, "frame_ms": 25, "shift_ms": 10, "window_type": "povey"},
            "max_segment_ms": 30000,
            "vad": {
                "model": "vad/stream-vad.onnx",
                "sha256": "0".repeat(64),
                "inputs": [{"name": "feat", "dtype": "float32", "shape": [1, -1, 80]}],
                "outputs": [{"name": "probs", "dtype": "float32", "shape": [1, -1, 1]}]
            },
            "backend": {"kind": "worker", "runtime": runtime, "device": "auto",
                         "min_vram_mb": 6144, "supports_partial": false,
                         "load_timeout_ms": 120000, "request_timeout_ms": 60000},
            "asr": {
                "kind": "fire-red-aed",
                "encoder": {"model": "asr/encoder.fp16.onnx", "sha256": "0".repeat(64),
                             "inputs": [{"name": "x", "dtype": "float32", "shape": [1, -1, 80]},
                                        {"name": "x_lens", "dtype": "int64", "shape": [1]}],
                             "outputs": [{"name": "cross_k", "dtype": "float32", "shape": [-1, -1, -1, -1]},
                                         {"name": "cross_v", "dtype": "float32", "shape": [-1, -1, -1, -1]}]},
                "decoder": {"model": "asr/decoder.fp16.onnx", "sha256": "0".repeat(64),
                             "inputs": [{"name": "tokens", "dtype": "int64", "shape": [1, 1]}],
                             "outputs": [{"name": "logits", "dtype": "float32", "shape": [1, 1, 8667]}]},
                "precision": "fp16",
                "tokens": "asr/tokens.txt",
                "token_count": 8667,
                "options": {"punc": "external", "partial": false}
            },
            "punc": {
                "model": {"model": "punc/model.onnx", "sha256": "0".repeat(64),
                           "inputs": [{"name": "input_ids", "dtype": "int64", "shape": [1, -1]}],
                           "outputs": [{"name": "logits", "dtype": "float32", "shape": [1, -1, -1]}]},
                "vocab": "punc/vocab.txt",
                "labels": "punc/labels.json",
                "max_tokens": 400
            }
        })
    }

    #[test]
    fn accepts_documented_aed_worker_package() {
        let manifest: ModelManifest =
            serde_json::from_value(aed_manifest("onnx-migraphx")).unwrap();
        manifest.validate().unwrap();
    }

    #[test]
    fn aed_requires_sense_voice_base_package() {
        let mut value = aed_manifest("onnx-migraphx");
        value["requires"] = serde_json::json!([]);
        let manifest: ModelManifest = serde_json::from_value(value).unwrap();
        assert!(manifest.validate().is_err());
    }

    #[test]
    fn aed_rejects_non_fp16_precision() {
        let mut value = aed_manifest("onnx-migraphx");
        value["asr"]["precision"] = serde_json::json!("int8");
        assert!(serde_json::from_value::<ModelManifest>(value).is_err());
    }
}
