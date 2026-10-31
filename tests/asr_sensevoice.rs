use std::path::{Path, PathBuf};

use voime_core::asr::backend::AsrBackend;
use voime_core::asr::features::{Fbank, FbankConfig};
use voime_core::infer::session::SessionFactory;
use voime_core::model::{ModelManifest, PuncPolicy, MANIFEST_FILE};
use voime_core::punc::firered::{FireRedPunc, PuncConfig};

fn read_wav(path: &Path) -> anyhow::Result<Vec<f32>> {
    let bytes = std::fs::read(path)?;
    if bytes.len() < 12 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        anyhow::bail!("不是 RIFF/WAVE 文件：{}", path.display());
    }
    let mut offset = 12usize;
    let mut format = None;
    let mut data = None;
    while offset + 8 <= bytes.len() {
        let id = &bytes[offset..offset + 4];
        let size = u32::from_le_bytes(bytes[offset + 4..offset + 8].try_into()?) as usize;
        let start = offset + 8;
        let end = start.saturating_add(size).min(bytes.len());
        match id {
            b"fmt " => format = Some(&bytes[start..end]),
            b"data" => data = Some(&bytes[start..end]),
            _ => {}
        }
        offset = end + (size & 1);
    }
    let format = format.ok_or_else(|| anyhow::anyhow!("WAV 缺少 fmt 块"))?;
    let data = data.ok_or_else(|| anyhow::anyhow!("WAV 缺少 data 块"))?;
    let channels = u16::from_le_bytes(format[2..4].try_into()?) as usize;
    let sample_rate = u32::from_le_bytes(format[4..8].try_into()?);
    let bits = u16::from_le_bytes(format[14..16].try_into()?) as usize;
    anyhow::ensure!(sample_rate == 16_000, "采样率不是 16 kHz");
    anyhow::ensure!(channels == 1, "测试音频不是单声道");
    anyhow::ensure!(bits == 16, "测试音频不是 PCM16");
    Ok(data
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| i16::from_le_bytes(*pair) as f32 / 32768.0)
        .collect())
}

/// Optional real-model test. Set `VOIME_SENSEVOICE_MODEL_DIR` to an installed
/// SenseVoice package and `VOIME_SENSEVOICE_AUDIO_DIR` to a directory of WAV
/// files. Without both variables the test is a no-op.
#[test]
fn sense_voice_transcribes_package_samples_when_available() {
    let Some(dir) = std::env::var_os("VOIME_SENSEVOICE_MODEL_DIR").map(PathBuf::from) else {
        return;
    };
    let Some(audio_dir) = std::env::var_os("VOIME_SENSEVOICE_AUDIO_DIR").map(PathBuf::from) else {
        return;
    };
    let mut manifest = ModelManifest::load(&dir.join(MANIFEST_FILE)).expect("manifest.json");
    manifest
        .verify(&dir)
        .expect("model package must match manifest");
    manifest.rebase(&dir).expect("rebase model paths");
    assert_eq!(
        manifest.fbank.window_type,
        voime_core::asr::features::WindowType::Hamming
    );
    let factory = SessionFactory::new(2).expect("ORT");
    let mut asr =
        AsrBackend::new_with_factory(&manifest.asr, &factory).expect("SenseVoice session");
    let mut punc = manifest
        .punc
        .as_ref()
        .filter(|_| manifest.asr.options.punc == PuncPolicy::External)
        .map(|artifact| {
            FireRedPunc::new_with_factory(
                artifact,
                &factory,
                PuncConfig {
                    max_tokens: artifact.max_tokens,
                },
            )
        })
        .transpose()
        .expect("FireRedPunc session");
    let mut fbank = Fbank::new(FbankConfig {
        sample_rate: manifest.sample_rate,
        num_bins: manifest.fbank.num_bins,
        frame_ms: manifest.fbank.frame_ms,
        shift_ms: manifest.fbank.shift_ms,
        window_type: manifest.fbank.window_type,
    });
    let mut wavs: Vec<PathBuf> = std::fs::read_dir(&audio_dir)
        .expect("audio dir")
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "wav"))
        .collect();
    wavs.sort();
    assert!(!wavs.is_empty(), "audio dir has no wav files");
    for wav in wavs {
        let samples = read_wav(&wav).expect("read wav");
        let features = fbank.compute(&samples).expect("fbank");
        let output = asr.transcribe(&features).expect("SenseVoice inference");
        println!("{} => {}", wav.display(), output.text);
        assert!(
            !output.text.trim().is_empty(),
            "empty transcript: {}",
            wav.display()
        );
        if let Some(punc) = punc.as_mut() {
            let punctuated = punc.punctuate(&output.text).expect("FireRedPunc");
            println!("  punctuated => {punctuated}");
            assert!(!punctuated.trim().is_empty());
        }
    }
}
