use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context};
use voime_ipc::Backend;

use crate::model::{
    default_pack_root, firered_pack_root, FireRedManifest, PuncManifest, SenseVoiceManifest,
    VadManifest, FIRE_RED_AED_DIR, MANIFEST_FILE, PUNC_DIR, SENSE_VOICE_DIR, VAD_DIR,
};

pub const DEFAULT_THREADS: usize = 2;
pub const DEFAULT_PRE_ROLL_MS: u32 = 250;
/// Single-line backend selection below the model root. Missing means CPU.
pub const BACKEND_FILE: &str = "backend";

#[derive(Clone, Debug)]
pub struct FireRedConfig {
    pub dir: PathBuf,
    pub manifest: FireRedManifest,
}

#[derive(Clone, Debug)]
pub struct PuncConfig {
    pub dir: PathBuf,
    pub manifest: PuncManifest,
}

/// Resolved model layout. The default pack (VAD + SenseVoice) is required for
/// every backend. The FireRed pack (AED + Punc) is only loaded when the
/// selected backend is GPU.
#[derive(Clone, Debug)]
pub struct Config {
    pub model_root: PathBuf,
    pub backend: Backend,
    pub default_pack: PathBuf,
    pub firered_pack: PathBuf,
    pub input_device: Option<String>,
    pub threads: usize,
    pub preview_enabled: bool,
    pub pre_roll_ms: u32,
    pub vad_dir: PathBuf,
    pub vad_manifest: VadManifest,
    pub sense_voice_dir: PathBuf,
    pub sense_voice_manifest: SenseVoiceManifest,
    pub fire_red: Option<FireRedConfig>,
    pub punc: Option<PuncConfig>,
}

impl Config {
    pub fn gpu_final(&self) -> bool {
        self.backend == Backend::Gpu
    }

    pub fn max_segment_ms(&self) -> u32 {
        match (self.backend, self.fire_red.as_ref()) {
            (Backend::Gpu, Some(config)) => config.manifest.max_segment_ms,
            _ => self.sense_voice_manifest.max_segment_ms,
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct Args {
    pub backend: Option<Backend>,
    pub input_device: Option<String>,
    pub threads: Option<usize>,
    pub verbose: bool,
}

pub fn resolve(args: Args) -> anyhow::Result<Config> {
    resolve_at(&default_model_root(), args)
}

pub fn resolve_at(root: &Path, args: Args) -> anyhow::Result<Config> {
    let threads = args.threads.unwrap_or(DEFAULT_THREADS);
    if threads == 0 {
        bail!("线程数必须大于 0");
    }

    let backend = match args.backend {
        Some(backend) => backend,
        None => read_backend(root)?.unwrap_or_default(),
    };

    let default_pack = default_pack_root(root);
    let vad_dir = default_pack.join(VAD_DIR);
    let mut vad_manifest = VadManifest::load(&vad_dir.join(MANIFEST_FILE))
        .with_context(|| format!("加载 VAD 清单失败 {}", vad_dir.display()))?;
    vad_manifest
        .verify(&vad_dir)
        .with_context(|| format!("校验 VAD 失败 {}", vad_dir.display()))?;
    vad_manifest
        .rebase(&vad_dir)
        .with_context(|| format!("解析 VAD 路径失败 {}", vad_dir.display()))?;

    let sense_voice_dir = default_pack.join(SENSE_VOICE_DIR);
    let mut sense_voice_manifest =
        SenseVoiceManifest::load(&sense_voice_dir.join(MANIFEST_FILE))
            .with_context(|| format!("加载 SenseVoice 清单失败 {}", sense_voice_dir.display()))?;
    sense_voice_manifest
        .verify(&sense_voice_dir)
        .with_context(|| format!("校验 SenseVoice 失败 {}", sense_voice_dir.display()))?;
    sense_voice_manifest
        .rebase(&sense_voice_dir)
        .with_context(|| format!("解析 SenseVoice 路径失败 {}", sense_voice_dir.display()))?;

    let firered_pack = firered_pack_root(root);
    let (fire_red, punc) = match backend {
        Backend::Cpu => (None, None),
        Backend::Gpu => {
            let (fire_red, punc) = load_firered_pack(root)?;
            (Some(fire_red), Some(punc))
        }
    };

    Ok(Config {
        model_root: root.to_path_buf(),
        backend,
        default_pack,
        firered_pack,
        input_device: args.input_device,
        threads,
        preview_enabled: true,
        pre_roll_ms: DEFAULT_PRE_ROLL_MS,
        vad_dir,
        vad_manifest,
        sense_voice_dir,
        sense_voice_manifest,
        fire_red,
        punc,
    })
}

/// Loads and verifies the FireRed pack. Used at startup and when the user
/// switches the running backend from CPU to GPU.
pub fn load_firered_pack(root: &Path) -> anyhow::Result<(FireRedConfig, PuncConfig)> {
    let pack = firered_pack_root(root);
    let fire_red_dir = pack.join(FIRE_RED_AED_DIR);
    let punc_dir = pack.join(PUNC_DIR);
    let fire_red_present = fire_red_dir.join(MANIFEST_FILE).is_file();
    let punc_present = punc_dir.join(MANIFEST_FILE).is_file();
    match (fire_red_present, punc_present) {
        (false, false) => bail!(
            "GPU 后端需要 firered 资源包：{}（运行安装脚本并追加 --gpu）",
            pack.display()
        ),
        (true, false) => bail!(
            "firered 资源包不完整：缺少 {}；FireRed AED 与 Punc 必须同时安装",
            punc_dir.display()
        ),
        (false, true) => bail!(
            "firered 资源包不完整：缺少 {}；FireRed AED 与 Punc 必须同时安装",
            fire_red_dir.display()
        ),
        (true, true) => {
            let mut manifest = FireRedManifest::load(&fire_red_dir.join(MANIFEST_FILE))
                .with_context(|| format!("加载 FireRed 清单失败 {}", fire_red_dir.display()))?;
            manifest
                .verify(&fire_red_dir)
                .with_context(|| format!("校验 FireRed 失败 {}", fire_red_dir.display()))?;
            manifest
                .rebase(&fire_red_dir)
                .with_context(|| format!("解析 FireRed 路径失败 {}", fire_red_dir.display()))?;

            let mut punc = PuncManifest::load(&punc_dir.join(MANIFEST_FILE))
                .with_context(|| format!("加载标点清单失败 {}", punc_dir.display()))?;
            punc.verify(&punc_dir)
                .with_context(|| format!("校验标点失败 {}", punc_dir.display()))?;
            punc.rebase(&punc_dir)
                .with_context(|| format!("解析标点路径失败 {}", punc_dir.display()))?;

            Ok((
                FireRedConfig {
                    dir: fire_red_dir,
                    manifest,
                },
                PuncConfig {
                    dir: punc_dir,
                    manifest: punc,
                },
            ))
        }
    }
}

pub fn backend_file(root: &Path) -> PathBuf {
    root.join(BACKEND_FILE)
}

/// Reads the persisted backend selection. A missing or blank file means the
/// caller should use the default (CPU).
pub fn read_backend(root: &Path) -> anyhow::Result<Option<Backend>> {
    let path = backend_file(root);
    let content = match fs::read(&path) {
        Ok(content) => content,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(error).with_context(|| format!("读取后端设置失败 {}", path.display()));
        }
    };
    let text = String::from_utf8(content)
        .with_context(|| format!("后端设置不是 UTF-8：{}", path.display()))?;
    let text = text.trim_start_matches('\u{feff}').trim();
    if text.is_empty() {
        return Ok(None);
    }
    if text.chars().any(char::is_control) {
        bail!("后端设置只能包含单行 cpu 或 gpu：{}", path.display());
    }
    let backend = text
        .parse::<Backend>()
        .map_err(|error| anyhow::anyhow!("{error}（{}）", path.display()))?;
    Ok(Some(backend))
}

/// Atomically persists the backend selection for the next daemon start.
pub fn write_backend(root: &Path, backend: Backend) -> anyhow::Result<()> {
    fs::create_dir_all(root).with_context(|| format!("创建模型根目录失败 {}", root.display()))?;
    let path = backend_file(root);
    let temporary = root.join(format!(".{BACKEND_FILE}.tmp-{}", std::process::id()));
    let write = (|| -> anyhow::Result<()> {
        let mut file = File::create(&temporary)
            .with_context(|| format!("创建临时后端设置失败 {}", temporary.display()))?;
        file.write_all(backend.as_str().as_bytes())
            .with_context(|| format!("写入临时后端设置失败 {}", temporary.display()))?;
        file.write_all(b"\n")
            .with_context(|| format!("写入临时后端设置失败 {}", temporary.display()))?;
        file.sync_all()
            .with_context(|| format!("同步临时后端设置失败 {}", temporary.display()))?;
        Ok(())
    })();
    if let Err(error) = write {
        let _ = fs::remove_file(&temporary);
        return Err(error);
    }
    fs::rename(&temporary, &path).with_context(|| {
        let _ = fs::remove_file(&temporary);
        format!("替换后端设置失败 {}", path.display())
    })
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

#[cfg(test)]
mod tests {
    use super::*;
    use sha2::{Digest, Sha256};

    fn sha256(data: &[u8]) -> String {
        format!("{:x}", Sha256::digest(data))
    }

    fn artifact(name: &str, sha: &str) -> serde_json::Value {
        serde_json::json!({
            "model": name,
            "sha256": sha,
            "inputs": [{"name": "x", "dtype": "float32", "shape": [1, -1, 80]}],
            "outputs": [{"name": "y", "dtype": "float32", "shape": [1, -1, 1]}]
        })
    }

    fn write_default_pack(root: &Path) {
        let default = default_pack_root(root);
        let vad = default.join(VAD_DIR);
        fs::create_dir_all(&vad).unwrap();
        fs::write(vad.join("stream-vad.onnx"), b"vad").unwrap();
        fs::write(
            vad.join(MANIFEST_FILE),
            serde_json::to_vec(&serde_json::json!({
                "schema": 3,
                "kind": "vad",
                "fbank": {"num_bins": 80, "frame_ms": 25, "shift_ms": 10, "window_type": "hamming"},
                "model": artifact("stream-vad.onnx", &sha256(b"vad"))
            }))
            .unwrap(),
        )
        .unwrap();

        let sense = default.join(SENSE_VOICE_DIR);
        fs::create_dir_all(&sense).unwrap();
        fs::write(sense.join("model.int8.onnx"), b"sense").unwrap();
        fs::write(sense.join("tokens.txt"), b"token 0\n").unwrap();
        fs::write(
            sense.join(MANIFEST_FILE),
            serde_json::to_vec(&serde_json::json!({
                "schema": 3,
                "kind": "sense-voice",
                "fbank": {"num_bins": 80, "frame_ms": 25, "shift_ms": 10, "window_type": "hamming"},
                "max_segment_ms": 20000,
                "model": artifact("model.int8.onnx", &sha256(b"sense")),
                "tokens": "tokens.txt",
                "token_count": 25055,
                "options": {"language": "auto", "use_itn": true, "partial": true}
            }))
            .unwrap(),
        )
        .unwrap();
    }

    fn write_firered_pack(root: &Path) {
        let firered = firered_pack_root(root);
        let aed = firered.join(FIRE_RED_AED_DIR);
        fs::create_dir_all(&aed).unwrap();
        fs::write(aed.join("encoder.fp16.onnx"), b"encoder").unwrap();
        fs::write(aed.join("decoder.fp16.onnx"), b"decoder").unwrap();
        fs::write(aed.join("tokens.txt"), b"token 0\n").unwrap();
        let encoder = artifact("encoder.fp16.onnx", &sha256(b"encoder"));
        let decoder = artifact("decoder.fp16.onnx", &sha256(b"decoder"));
        fs::write(
            aed.join(MANIFEST_FILE),
            serde_json::to_vec(&serde_json::json!({
                "schema": 3,
                "kind": "fire-red-aed",
                "fbank": {"num_bins": 80, "frame_ms": 25, "shift_ms": 10, "window_type": "povey"},
                "max_segment_ms": 30000,
                "encoder": encoder,
                "decoder": decoder,
                "precision": "fp16",
                "tokens": "tokens.txt",
                "token_count": 7832
            }))
            .unwrap(),
        )
        .unwrap();

        let punc = firered.join(PUNC_DIR);
        fs::create_dir_all(&punc).unwrap();
        fs::write(punc.join("model.onnx"), b"punc").unwrap();
        fs::write(punc.join("vocab.txt"), b"vocab\n").unwrap();
        fs::write(punc.join("labels.json"), b"[]\n").unwrap();
        fs::write(
            punc.join(MANIFEST_FILE),
            serde_json::to_vec(&serde_json::json!({
                "schema": 3,
                "kind": "punc",
                "model": artifact("model.onnx", &sha256(b"punc")),
                "vocab": "vocab.txt",
                "labels": "labels.json",
                "max_tokens": 400
            }))
            .unwrap(),
        )
        .unwrap();
    }

    #[test]
    fn rejects_zero_threads_before_touching_disk() {
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
    fn cpu_backend_only_needs_the_default_pack() {
        let directory = tempfile::tempdir().unwrap();
        write_default_pack(directory.path());
        let config = resolve_at(
            directory.path(),
            Args {
                backend: Some(Backend::Cpu),
                ..Args::default()
            },
        )
        .unwrap();
        assert_eq!(config.backend, Backend::Cpu);
        assert!(!config.gpu_final());
        assert!(config.fire_red.is_none());
        assert!(config.punc.is_none());
        assert!(config.default_pack.ends_with("default"));
    }

    #[test]
    fn gpu_backend_requires_the_firered_pack() {
        let directory = tempfile::tempdir().unwrap();
        write_default_pack(directory.path());
        let error = resolve_at(
            directory.path(),
            Args {
                backend: Some(Backend::Gpu),
                ..Args::default()
            },
        )
        .unwrap_err();
        assert!(error.to_string().contains("firered"));

        write_firered_pack(directory.path());
        let config = resolve_at(
            directory.path(),
            Args {
                backend: Some(Backend::Gpu),
                ..Args::default()
            },
        )
        .unwrap();
        assert_eq!(config.backend, Backend::Gpu);
        assert!(config.gpu_final());
        assert!(config.fire_red.is_some());
        assert!(config.punc.is_some());
        assert_eq!(config.max_segment_ms(), 30000);
    }

    #[test]
    fn backend_file_defaults_to_none_and_round_trips() {
        let directory = tempfile::tempdir().unwrap();
        assert_eq!(read_backend(directory.path()).unwrap(), None);
        write_backend(directory.path(), Backend::Gpu).unwrap();
        assert_eq!(read_backend(directory.path()).unwrap(), Some(Backend::Gpu));
        write_backend(directory.path(), Backend::Cpu).unwrap();
        assert_eq!(read_backend(directory.path()).unwrap(), Some(Backend::Cpu));
    }

    #[test]
    fn backend_file_is_used_when_args_do_not_override() {
        let directory = tempfile::tempdir().unwrap();
        write_default_pack(directory.path());
        write_firered_pack(directory.path());
        write_backend(directory.path(), Backend::Gpu).unwrap();
        let config = resolve_at(directory.path(), Args::default()).unwrap();
        assert_eq!(config.backend, Backend::Gpu);
    }

    #[test]
    fn rejects_invalid_backend_file() {
        let directory = tempfile::tempdir().unwrap();
        fs::write(backend_file(directory.path()), "npu\n").unwrap();
        assert!(read_backend(directory.path()).is_err());
    }

    #[test]
    fn pack_roots_are_single_directories() {
        let root = Path::new("/models");
        assert_eq!(default_pack_root(root), root.join("default"));
        assert_eq!(firered_pack_root(root), root.join("firered"));
    }
}
