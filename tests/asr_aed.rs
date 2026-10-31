mod common;

use std::path::PathBuf;

use common::read_wav;
use voime_core::asr::backend::AsrBackend;
use voime_core::asr::features::{Fbank, FbankConfig};
use voime_core::infer::session::SessionFactory;
use voime_core::model::{ModelManifest, PuncPolicy, MANIFEST_FILE};
use voime_core::punc::firered::{FireRedPunc, PuncConfig};

/// Optional real-model test. Set `VOIME_AED_MODEL_DIR` to an installed
/// FireRed AED package and `VOIME_AED_AUDIO_DIR` to a directory of WAV files.
/// This loads two large ONNX graphs and is intentionally not part of the
/// default offline CI.
#[test]
fn fire_red_aed_transcribes_package_samples_when_available() {
    let Some(dir) = std::env::var_os("VOIME_AED_MODEL_DIR").map(PathBuf::from) else {
        return;
    };
    let Some(audio_dir) = std::env::var_os("VOIME_AED_AUDIO_DIR").map(PathBuf::from) else {
        return;
    };
    let mut manifest = ModelManifest::load(&dir.join(MANIFEST_FILE)).expect("manifest.json");
    manifest
        .verify(&dir)
        .expect("model package must match manifest");
    manifest.rebase(&dir).expect("rebase model paths");
    assert_eq!(manifest.asr.kind, voime_core::model::AsrKind::FireRedAed);
    assert!(!manifest.asr.partial_enabled());
    let factory = SessionFactory::new(2).expect("ORT");
    let mut asr = AsrBackend::new_with_factory(&manifest.asr, &factory).expect("AED sessions");
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
        let samples = match read_wav(&wav) {
            Ok(samples) => samples,
            Err(error) if format!("{error:#}").contains("采样率") => {
                println!("{} => skip（非 16 kHz 样例）", wav.display());
                continue;
            }
            Err(error) => panic!("read {}: {error:#}", wav.display()),
        };
        let features = fbank.compute(&samples).expect("fbank");
        let output = asr.transcribe(&features).expect("AED inference");
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
