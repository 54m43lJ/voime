use std::path::{Path, PathBuf};

use anyhow::{bail, Context};

use crate::model::{
    FireRedManifest, PuncManifest, SenseVoiceManifest, VadManifest, FIRE_RED_AED_DIR,
    MANIFEST_FILE, PUNC_DIR, SENSE_VOICE_DIR, VAD_DIR,
};

pub const DEFAULT_THREADS: usize = 2;
pub const DEFAULT_PRE_ROLL_MS: u32 = 250;

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

/// Fixed model layout. There is no active pointer or package id: the daemon
/// always loads VAD + SenseVoice, and uses the optional FireRed AED + Punc
/// directories as the GPU Final path when both are installed.
#[derive(Clone, Debug)]
pub struct Config {
    pub model_root: PathBuf,
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
        self.fire_red.is_some() && self.punc.is_some()
    }

    pub fn max_segment_ms(&self) -> u32 {
        self.fire_red
            .as_ref()
            .map(|config| config.manifest.max_segment_ms)
            .unwrap_or(self.sense_voice_manifest.max_segment_ms)
    }
}

#[derive(Clone, Debug, Default)]
pub struct Args {
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

    let vad_dir = root.join(VAD_DIR);
    let mut vad_manifest = VadManifest::load(&vad_dir.join(MANIFEST_FILE))
        .with_context(|| format!("加载 VAD 清单失败 {}", vad_dir.display()))?;
    vad_manifest
        .verify(&vad_dir)
        .with_context(|| format!("校验 VAD 失败 {}", vad_dir.display()))?;
    vad_manifest
        .rebase(&vad_dir)
        .with_context(|| format!("解析 VAD 路径失败 {}", vad_dir.display()))?;

    let sense_voice_dir = root.join(SENSE_VOICE_DIR);
    let mut sense_voice_manifest =
        SenseVoiceManifest::load(&sense_voice_dir.join(MANIFEST_FILE))
            .with_context(|| format!("加载 SenseVoice 清单失败 {}", sense_voice_dir.display()))?;
    sense_voice_manifest
        .verify(&sense_voice_dir)
        .with_context(|| format!("校验 SenseVoice 失败 {}", sense_voice_dir.display()))?;
    sense_voice_manifest
        .rebase(&sense_voice_dir)
        .with_context(|| format!("解析 SenseVoice 路径失败 {}", sense_voice_dir.display()))?;

    let fire_red_dir = root.join(FIRE_RED_AED_DIR);
    let punc_dir = root.join(PUNC_DIR);
    let fire_red_present = fire_red_dir.join(MANIFEST_FILE).is_file();
    let punc_present = punc_dir.join(MANIFEST_FILE).is_file();
    let (fire_red, punc) = match (fire_red_present, punc_present) {
        (false, false) => (None, None),
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
            (
                Some(FireRedConfig {
                    dir: fire_red_dir,
                    manifest,
                }),
                Some(PuncConfig {
                    dir: punc_dir,
                    manifest: punc,
                }),
            )
        }
        (true, false) => bail!(
            "FireRed Final 不完整：缺少 {}；默认只安装 CPU 档，GPU 档需要同时安装 FireRed 与 punc",
            punc_dir.display()
        ),
        (false, true) => bail!(
            "标点模型多余或 FireRed Final 不完整：缺少 {}",
            fire_red_dir.display()
        ),
    };

    Ok(Config {
        model_root: root.to_path_buf(),
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
}
