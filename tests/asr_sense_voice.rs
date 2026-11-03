use std::path::PathBuf;

use voime_core::asr::r#final::{FinalBackend, SenseVoiceBackend};
use voime_core::asr::sense_voice::{greedy_decode, parse_cmvn};
use voime_core::asr::tokenizer::AsrTokenizer;
use voime_core::infer::session::SessionFactory;
use voime_core::model::{SenseVoiceManifest, DEFAULT_PACK_DIR, MANIFEST_FILE, SENSE_VOICE_DIR};
use voime_core::pipeline::fbank_config;

#[test]
fn tokenizer_blank_and_space_handling() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("tokens.txt");
    std::fs::write(
        &path,
        "<blank> 0\n\u{2581}hello 1\n\u{2581}world 2\n\u{2581} 3\n",
    )
    .unwrap();
    let tokenizer = AsrTokenizer::load(&path).unwrap();
    assert_eq!(tokenizer.decode(&[0, 1, 3, 2]), "hello world");
}

#[test]
fn greedy_decode_matches_ctc_rules() {
    let logits = vec![
        0.0, 0.9, 0.1, // class 1
        0.0, 0.9, 0.1, // repeated
        0.0, 0.0, 1.0, // class 2
        1.0, 0.0, 0.0, // blank
        0.0, 0.0, 1.0, // class 2 again
    ];
    assert_eq!(greedy_decode(&logits, 5, 3), vec![1, 2, 2]);
}

#[test]
fn cmvn_metadata_encodings() {
    assert_eq!(parse_cmvn("[1.5,-2,0]").unwrap(), vec![1.5, -2.0, 0.0]);
    assert_eq!(parse_cmvn("1.5 -2 0").unwrap(), vec![1.5, -2.0, 0.0]);
}

/// Optional real-model smoke test. Set VOIME_ASR_MODEL_DIR to the installed
/// SenseVoice package to decode 10–60 s of silence through the real ONNX
/// graph. Without it the test is a no-op so normal `cargo test` stays offline.
#[test]
fn sense_voice_decodes_10_to_60_seconds_when_model_available() {
    let Some(root) = std::env::var_os("VOIME_ASR_MODEL_DIR").map(PathBuf::from) else {
        return;
    };
    let dir = if root.join(MANIFEST_FILE).is_file() {
        root
    } else if root.join(DEFAULT_PACK_DIR).join(SENSE_VOICE_DIR).is_dir() {
        root.join(DEFAULT_PACK_DIR).join(SENSE_VOICE_DIR)
    } else {
        root.join(SENSE_VOICE_DIR)
    };
    let mut manifest = SenseVoiceManifest::load(&dir.join(MANIFEST_FILE)).expect("manifest.json");
    manifest
        .verify(&dir)
        .expect("model directory must match manifest");
    manifest.rebase(&dir).expect("rebase model paths");
    let factory = SessionFactory::new(2).expect("ORT");
    let mut backend =
        SenseVoiceBackend::new_with_factory(&manifest, &factory, fbank_config(&manifest.fbank))
            .expect("SenseVoice session");
    for seconds in [10usize, 20, 30, 60] {
        let samples = vec![0.0f32; seconds * 16_000];
        let output = backend.transcribe(&samples).expect("SenseVoice inference");
        assert!(output.text.chars().count() < 4096);
    }
}
