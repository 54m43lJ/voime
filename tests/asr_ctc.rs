use std::path::PathBuf;

use voime_core::asr::features::{Fbank, FbankConfig, WindowType};
use voime_core::asr::firered_ctc::{greedy_decode, parse_cmvn, FireRedCtc};
use voime_core::asr::tokenizer::CtcTokenizer;
use voime_core::infer::session::SessionFactory;
use voime_core::model::{ModelManifest, MANIFEST_FILE};

#[test]
fn tokenizer_blank_and_space_handling() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("tokens.txt");
    std::fs::write(
        &path,
        "<blank> 0\n\u{2581}hello 1\n\u{2581}world 2\n\u{2581} 3\n",
    )
    .unwrap();
    let tokenizer = CtcTokenizer::load(&path).unwrap();
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

/// Optional real-model smoke test. Set VOIME_CTC_MODEL_DIR to the installed
/// model package to decode 10/20/30/60 s of silence through the real ONNX
/// graph. Without it the test is a no-op so normal `cargo test` stays offline.
#[test]
fn ctc_decodes_10_to_60_seconds_when_model_available() {
    let Some(dir) = std::env::var_os("VOIME_CTC_MODEL_DIR").map(PathBuf::from) else {
        return;
    };
    let mut manifest = ModelManifest::load(&dir.join(MANIFEST_FILE)).expect("manifest.json");
    manifest
        .verify(&dir)
        .expect("model package must match manifest");
    manifest.rebase(&dir).expect("rebase model paths");
    let factory = SessionFactory::new(2).expect("ORT");
    let mut ctc = FireRedCtc::new_with_factory(&manifest.asr, &factory).expect("CTC session");
    let mut fbank = Fbank::new(FbankConfig {
        sample_rate: manifest.sample_rate,
        num_bins: manifest.fbank.num_bins,
        frame_ms: manifest.fbank.frame_ms,
        shift_ms: manifest.fbank.shift_ms,
        window_type: manifest.fbank.window_type,
    });
    assert_eq!(manifest.fbank.window_type, WindowType::Povey);
    for seconds in [10usize, 20, 30, 60] {
        let samples = vec![0.0f32; seconds * 16_000];
        let features = fbank.compute(&samples).expect("fbank");
        let output = ctc.transcribe(&features).expect("CTC inference");
        assert!(output.text.len() < 1024);
    }
}
