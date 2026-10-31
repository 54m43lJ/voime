use std::fs::{self, File};
use std::io::{BufReader, Read, Write};
use std::path::{Component, Path, PathBuf};

use anyhow::{bail, Context};
use serde::Deserialize;
use sha2::{Digest, Sha256};

pub const MANIFEST_FILE: &str = "manifest.json";
pub const ACTIVE_FILE: &str = "active";
/// Reserved active value that means "the model root itself is one package".
/// It keeps pre-package installs working without a migration.
pub const LEGACY_PACKAGE_ID: &str = "legacy";
pub const SCHEMA_VERSION: u32 = 2;
pub const SAMPLES_PER_SECOND: u32 = 16_000;

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

/// ASR implementation selected by a model package. All variants are compiled
/// into the same daemon binary; switching never replaces the executable.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub enum AsrKind {
    FireRedCtc,
    SenseVoice,
    FireRedAed,
}

impl AsrKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::FireRedCtc => "fire-red-ctc",
            Self::SenseVoice => "sense-voice",
            Self::FireRedAed => "fire-red-aed",
        }
    }

    pub fn supports_partial(self) -> bool {
        !matches!(self, Self::FireRedAed)
    }
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum PuncPolicy {
    /// The ASR output has no punctuation; FireRedPunc adds it.
    #[default]
    External,
    /// The ASR model already emits punctuation / ITN (SenseVoice `use_itn`).
    Builtin,
}

#[derive(Clone, Debug, Deserialize)]
pub struct AsrOptions {
    /// SenseVoice: auto / zh / en / ja / ko / yue. Other backends ignore it.
    #[serde(default)]
    pub language: Option<String>,
    /// SenseVoice text normalization switch. FireRed packages ignore it.
    #[serde(default)]
    pub use_itn: bool,
    #[serde(default)]
    pub punc: PuncPolicy,
    #[serde(default = "default_partial")]
    pub partial: bool,
}

impl Default for AsrOptions {
    fn default() -> Self {
        Self {
            language: None,
            use_itn: false,
            punc: PuncPolicy::External,
            partial: default_partial(),
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
pub struct AsrArtifact {
    #[serde(default = "default_asr_kind")]
    pub kind: AsrKind,
    /// FireRed CTC / SenseVoice single-graph package.
    #[serde(default)]
    pub model: Option<Artifact>,
    /// FireRed AED encoder graph.
    #[serde(default)]
    pub encoder: Option<Artifact>,
    /// FireRed AED decoder graph.
    #[serde(default)]
    pub decoder: Option<Artifact>,
    pub tokens: PathBuf,
    #[serde(default = "default_token_count")]
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

    /// Whether the package opts in to partial recognition and its backend can
    /// actually produce it.
    pub fn partial_enabled(&self) -> bool {
        self.options.partial && self.kind.supports_partial()
    }
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
    /// Human-readable name shown by model pickers; defaults to the package id.
    #[serde(default)]
    pub display_name: Option<String>,
    pub vad: VadArtifact,
    pub asr: AsrArtifact,
    /// Required for `punc = external`; omitted by packages with built-in
    /// punctuation (for example SenseVoice ITN).
    #[serde(default)]
    pub punc: Option<PuncArtifact>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct FbankManifest {
    #[serde(default = "default_num_bins")]
    pub num_bins: usize,
    #[serde(default = "default_frame_ms")]
    pub frame_ms: u32,
    #[serde(default = "default_shift_ms")]
    pub shift_ms: u32,
    #[serde(default)]
    pub window_type: crate::asr::features::WindowType,
}

impl Default for FbankManifest {
    fn default() -> Self {
        Self {
            num_bins: default_num_bins(),
            frame_ms: default_frame_ms(),
            shift_ms: default_shift_ms(),
            window_type: crate::asr::features::WindowType::default(),
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
    30_000
}

fn default_token_count() -> usize {
    8667
}

fn default_asr_kind() -> AsrKind {
    AsrKind::FireRedCtc
}

fn default_punc_max_tokens() -> usize {
    400
}

fn default_partial() -> bool {
    true
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
        if manifest.sample_rate != SAMPLES_PER_SECOND {
            bail!(
                "模型采样率 {} 与内部采样率 {} 不一致",
                manifest.sample_rate,
                SAMPLES_PER_SECOND
            );
        }
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
        let tokens = resolve_inside(dir, &self.asr.tokens)?;
        ensure_file(&tokens)?;
        if let Some(punc) = self.punc.as_ref() {
            for aux in [&punc.vocab, &punc.labels] {
                let path = resolve_inside(dir, aux)?;
                ensure_file(&path)?;
            }
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

    pub fn display_name(&self, package_id: &str) -> String {
        self.display_name
            .clone()
            .unwrap_or_else(|| package_id.to_string())
    }

    fn artifacts(&self) -> Vec<&Artifact> {
        let mut artifacts = vec![&self.vad];
        artifacts.extend(self.asr.artifacts());
        if let Some(punc) = self.punc.as_ref() {
            artifacts.push(&punc.model);
        }
        artifacts
    }

    fn validate(&self) -> anyhow::Result<()> {
        if let Some(display_name) = self.display_name.as_ref() {
            if display_name.trim().is_empty() {
                bail!("display_name 不能为空");
            }
            if display_name.chars().any(char::is_control) {
                bail!("display_name 不能包含控制字符");
            }
        }
        self.validate_kind()?;
        self.validate_options()?;
        self.validate_specs()?;
        Ok(())
    }

    fn validate_kind(&self) -> anyhow::Result<()> {
        match self.asr.kind {
            AsrKind::FireRedCtc | AsrKind::SenseVoice => {
                if self.asr.model.is_none() {
                    bail!("{} 包必须声明 asr.model", self.asr.kind.as_str());
                }
                if self.asr.encoder.is_some() || self.asr.decoder.is_some() {
                    bail!(
                        "{} 包不能声明 asr.encoder / asr.decoder",
                        self.asr.kind.as_str()
                    );
                }
            }
            AsrKind::FireRedAed => {
                if self.asr.model.is_some() {
                    bail!("fire-red-aed 包不能声明 asr.model");
                }
                if self.asr.encoder.is_none() || self.asr.decoder.is_none() {
                    bail!("fire-red-aed 包必须同时声明 asr.encoder 与 asr.decoder");
                }
            }
        }
        Ok(())
    }

    fn validate_options(&self) -> anyhow::Result<()> {
        match self.asr.kind {
            AsrKind::FireRedCtc | AsrKind::FireRedAed => {
                if self.asr.options.punc != PuncPolicy::External {
                    bail!("{} 只能使用 external 标点", self.asr.kind.as_str());
                }
                if self.asr.options.language.is_some() {
                    bail!("{} 不支持 asr.options.language", self.asr.kind.as_str());
                }
            }
            AsrKind::SenseVoice => {
                if let Some(language) = self.asr.options.language.as_deref() {
                    if !matches!(language, "auto" | "zh" | "en" | "ja" | "ko" | "yue") {
                        bail!("SenseVoice 不支持语言：{language}");
                    }
                }
                match self.asr.options.punc {
                    PuncPolicy::Builtin if !self.asr.options.use_itn => {
                        bail!("builtin 标点要求 use_itn = true");
                    }
                    PuncPolicy::External if self.asr.options.use_itn => {
                        bail!("external 标点要求 use_itn = false");
                    }
                    _ => {}
                }
            }
        }
        match self.asr.options.punc {
            PuncPolicy::External if self.punc.is_none() => {
                bail!("external 标点需要顶层 punc 段");
            }
            PuncPolicy::Builtin if self.punc.is_some() => {
                bail!("builtin 标点包不应携带 punc 段");
            }
            _ => {}
        }
        Ok(())
    }

    fn validate_specs(&self) -> anyhow::Result<()> {
        for artifact in self.artifacts() {
            let label = &artifact.model.display().to_string();
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
        if let Some(punc) = self.punc.as_ref() {
            if punc.max_tokens == 0 {
                bail!("punc.max_tokens 必须大于 0");
            }
        }
        if self.asr.token_count == 0 {
            bail!("asr.token_count 必须大于 0");
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

/// A locally installed package discovered under the model root.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InstalledModel {
    pub id: String,
    pub display_name: String,
    pub kind: AsrKind,
    pub dir: PathBuf,
}

/// Scans one level below `root` for packages and also exposes a legacy package
/// when `root/manifest.json` is present. Unreadable packages are skipped so a
/// broken download cannot hide the other installed models.
pub fn scan_models(root: &Path) -> Vec<InstalledModel> {
    let mut models = Vec::new();
    if root.join(MANIFEST_FILE).is_file() {
        if let Ok(manifest) = ModelManifest::load(&root.join(MANIFEST_FILE)) {
            models.push(InstalledModel {
                id: LEGACY_PACKAGE_ID.to_string(),
                display_name: manifest.display_name("旧布局"),
                kind: manifest.asr.kind,
                dir: root.to_path_buf(),
            });
        }
    }
    let Ok(entries) = fs::read_dir(root) else {
        return models;
    };
    for entry in entries.flatten() {
        let dir = entry.path();
        if !dir.is_dir() {
            continue;
        }
        let Some(id) = dir.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if id == LEGACY_PACKAGE_ID || validate_package_id(id).is_err() {
            continue;
        }
        let manifest_path = dir.join(MANIFEST_FILE);
        if !manifest_path.is_file() {
            continue;
        }
        let Ok(manifest) = ModelManifest::load(&manifest_path) else {
            continue;
        };
        models.push(InstalledModel {
            id: id.to_string(),
            display_name: manifest.display_name(id),
            kind: manifest.asr.kind,
            dir,
        });
    }
    models.sort_by(|left, right| {
        left.display_name
            .to_lowercase()
            .cmp(&right.display_name.to_lowercase())
            .then_with(|| left.id.cmp(&right.id))
    });
    models
}

/// Reads `<root>/active`. A missing file is not an error (old installs).
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

/// Validates and persists a selection. Only structural checks are performed;
/// full SHA-256 verification happens when the daemon restarts the backend.
pub fn select_package(root: &Path, package_id: &str) -> anyhow::Result<()> {
    validate_package_id(package_id)?;
    let dir = package_dir(root, package_id)?;
    if package_id != LEGACY_PACKAGE_ID && !dir.join(MANIFEST_FILE).is_file() {
        bail!("模型包不存在：{package_id}");
    }
    ModelManifest::load(&dir.join(MANIFEST_FILE))
        .with_context(|| format!("模型包清单无效：{package_id}"))?;
    write_active_id(root, package_id)
}

pub fn package_dir(root: &Path, package_id: &str) -> anyhow::Result<PathBuf> {
    validate_package_id(package_id)?;
    if package_id == LEGACY_PACKAGE_ID {
        Ok(root.to_path_buf())
    } else {
        resolve_inside(root, Path::new(package_id))
    }
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

    fn manifest_json(kind: &str, punc: bool, options: &str) -> String {
        format!(
            r#"{{
                "schema": 2,
                "sample_rate": 16000,
                "vad": {{
                    "model": "vad/stream-vad.onnx",
                    "sha256": "0000000000000000000000000000000000000000000000000000000000000000",
                    "inputs": [{{"name":"feat","dtype":"float32","shape":[1,-1,80]}}],
                    "outputs": [{{"name":"probs","dtype":"float32","shape":[1,-1,1]}}]
                }},
                "asr": {{
                    "kind": "{kind}",
                    "model": {{
                        "model": "asr/model.int8.onnx",
                        "sha256": "0000000000000000000000000000000000000000000000000000000000000000",
                        "inputs": [{{"name":"x","dtype":"float32","shape":[1,-1,80]}}],
                        "outputs": [{{"name":"log_probs","dtype":"float32","shape":[1,-1,8667]}}]
                    }},
                    "tokens": "asr/tokens.txt",
                    "options": {options}
                }}
                {punc}
            }}"#,
            punc = if punc {
                r#", "punc": {
                    "model": {
                        "model": "punc/model.onnx",
                        "sha256": "0000000000000000000000000000000000000000000000000000000000000000",
                        "inputs": [{"name":"input_ids","dtype":"int64","shape":[1,-1]}],
                        "outputs": [{"name":"logits","dtype":"float32","shape":[1,-1,-1]}]
                    },
                    "vocab": "punc/vocab.txt",
                    "labels": "punc/labels.json"
                }"#
            } else {
                ""
            }
        )
    }

    #[test]
    fn rejects_parent_directory_paths() {
        let dir = Path::new("/tmp/models");
        assert!(resolve_inside(dir, Path::new("../secret")).is_err());
        assert!(resolve_inside(dir, Path::new("/etc/passwd")).is_err());
        assert!(resolve_inside(dir, Path::new("vad/model.onnx")).is_ok());
    }

    #[test]
    fn schema_defaults_match_ctc_package() {
        let manifest: ModelManifest =
            serde_json::from_str(&manifest_json("fire-red-ctc", true, "{}")).unwrap();
        assert_eq!(manifest.asr.kind, AsrKind::FireRedCtc);
        assert_eq!(manifest.asr.token_count, 8667);
        assert_eq!(manifest.max_segment_ms, 30_000);
        assert_eq!(manifest.asr.options.punc, PuncPolicy::External);
        assert!(manifest.asr.options.partial);
        manifest.validate().unwrap();
    }

    #[test]
    fn sense_voice_builtin_punctuation_needs_itn() {
        let manifest: ModelManifest = serde_json::from_str(&manifest_json(
            "sense-voice",
            false,
            r#"{"language":"zh","use_itn":false,"punc":"builtin"}"#,
        ))
        .unwrap();
        assert!(manifest.validate().is_err());

        let manifest: ModelManifest = serde_json::from_str(&manifest_json(
            "sense-voice",
            false,
            r#"{"language":"zh","use_itn":true,"punc":"builtin"}"#,
        ))
        .unwrap();
        manifest.validate().unwrap();
    }

    #[test]
    fn external_punctuation_requires_punc_artifact() {
        let manifest: ModelManifest =
            serde_json::from_str(&manifest_json("fire-red-ctc", false, "{}")).unwrap();
        assert!(manifest.validate().is_err());
    }

    #[test]
    fn rejects_absolute_and_nested_package_ids() {
        assert!(validate_package_id("sense-voice-2024-itn").is_ok());
        assert!(validate_package_id("../escape").is_err());
        assert!(validate_package_id("/tmp/model").is_err());
        assert!(validate_package_id("nested/model").is_err());
        assert!(validate_package_id("legacy").is_ok());
    }

    #[test]
    fn active_pointer_accepts_bom_and_whitespace_but_rejects_multiline() {
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
    fn writes_active_atomically() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        write_active_id(root, "sense-voice-2024-itn").unwrap();
        assert_eq!(
            read_active_id(root).unwrap().as_deref(),
            Some("sense-voice-2024-itn")
        );
        write_active_id(root, LEGACY_PACKAGE_ID).unwrap();
        assert_eq!(
            read_active_id(root).unwrap().as_deref(),
            Some(LEGACY_PACKAGE_ID)
        );
    }
}
