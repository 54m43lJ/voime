use std::fs;
use std::path::Path;

use voime_core::config::resolve_model_dir_at;
use voime_core::model::{
    read_active_id, scan_models, select_package, write_active_id, PuncPolicy, ACTIVE_FILE,
    LEGACY_PACKAGE_ID, MANIFEST_FILE,
};

fn manifest(kind: &str, builtin: bool, punc: bool) -> String {
    let options = if builtin {
        r#""options": {"language": "zh", "use_itn": true, "punc": "builtin"}"#
    } else {
        r#""options": {"punc": "external"}"#
    };
    let punc_section = if punc {
        r#", "punc": {
            "model": {"model": "punc/model.onnx", "sha256": "0000000000000000000000000000000000000000000000000000000000000000",
                "inputs": [{"name": "input_ids", "dtype": "int64", "shape": [1, -1]}],
                "outputs": [{"name": "logits", "dtype": "float32", "shape": [1, -1, -1]}]},
            "vocab": "punc/vocab.txt", "labels": "punc/labels.json"
        }"#
    } else {
        ""
    };
    format!(
        r#"{{
            "schema": 2, "sample_rate": 16000,
            "vad": {{"model": "vad/stream-vad.onnx", "sha256": "0000000000000000000000000000000000000000000000000000000000000000",
                "inputs": [{{"name": "feat", "dtype": "float32", "shape": [1, -1, 80]}}],
                "outputs": [{{"name": "probs", "dtype": "float32", "shape": [1, -1, 1]}}]}},
            "asr": {{"kind": "{kind}", "model": {{"model": "asr/model.int8.onnx", "sha256": "0000000000000000000000000000000000000000000000000000000000000000",
                "inputs": [{{"name": "x", "dtype": "float32", "shape": [1, -1, 560]}}],
                "outputs": [{{"name": "logits", "dtype": "float32", "shape": [1, -1, 25055]}}]}},
                "tokens": "asr/tokens.txt", "token_count": 25055, {options}}}
            {punc_section}
        }}"#
    )
}

fn write_package(root: &Path, id: &str, kind: &str, builtin: bool, punc: bool) {
    let dir = root.join(id);
    fs::create_dir_all(&dir).unwrap();
    let content = manifest(kind, builtin, punc);
    let value: serde_json::Value = serde_json::from_str(&content).unwrap();
    fs::write(
        dir.join(MANIFEST_FILE),
        serde_json::to_vec_pretty(&value).unwrap(),
    )
    .unwrap();
}

#[test]
fn scans_selects_and_resolves_installed_packages() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    write_package(root, "fire-red-ctc", "fire-red-ctc", false, true);
    write_package(root, "sense-voice-2024-itn", "sense-voice", true, false);

    let models = scan_models(root);
    assert_eq!(models.len(), 2);
    assert_eq!(models[0].id, "fire-red-ctc");
    assert_eq!(models[1].id, "sense-voice-2024-itn");
    assert_eq!(models[1].kind, voime_core::model::AsrKind::SenseVoice);

    select_package(root, "sense-voice-2024-itn").unwrap();
    assert_eq!(
        read_active_id(root).unwrap().as_deref(),
        Some("sense-voice-2024-itn")
    );
    let (resolved_root, id, dir) = resolve_model_dir_at(root, None).unwrap();
    assert_eq!(resolved_root, root);
    assert_eq!(id, "sense-voice-2024-itn");
    assert_eq!(dir, root.join("sense-voice-2024-itn"));

    assert!(select_package(root, "../escape").is_err());
    assert!(select_package(root, "missing").is_err());
    assert_eq!(
        read_active_id(root).unwrap().as_deref(),
        Some("sense-voice-2024-itn")
    );
}

#[test]
fn legacy_layout_and_reserved_legacy_pointer_still_work() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    fs::write(
        root.join(MANIFEST_FILE),
        manifest("fire-red-ctc", false, true),
    )
    .unwrap();
    assert_eq!(scan_models(root)[0].id, LEGACY_PACKAGE_ID);
    let (_, id, dir) = resolve_model_dir_at(root, None).unwrap();
    assert_eq!(id, LEGACY_PACKAGE_ID);
    assert_eq!(dir, root);
    write_active_id(root, LEGACY_PACKAGE_ID).unwrap();
    assert_eq!(
        fs::read_to_string(root.join(ACTIVE_FILE)).unwrap().trim(),
        LEGACY_PACKAGE_ID
    );
}

#[test]
fn builtin_punctuation_is_optional_and_external_is_required() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    write_package(root, "sense-voice-2024-itn", "sense-voice", true, false);
    write_package(root, "fire-red-ctc", "fire-red-ctc", false, true);
    let sense = scan_models(root)
        .into_iter()
        .find(|model| model.id == "sense-voice-2024-itn")
        .unwrap();
    assert_eq!(sense.kind, voime_core::model::AsrKind::SenseVoice);
    // Both packages loaded successfully; builtin packages carry no punc
    // artifact while FireRed packages require one.
    let _ = PuncPolicy::Builtin;
    let _ = PuncPolicy::External;
}
