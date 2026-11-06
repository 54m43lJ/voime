use std::fs;

use sha2::{Digest, Sha256};
use voime_core::model::{load_models, ModelManifest, MANIFEST_FILE};

fn sha256(data: &[u8]) -> String {
    format!("{:x}", Sha256::digest(data))
}

fn write_manifest(dir: &std::path::Path, id: &str, kind: &str, file: &str, data: &[u8]) {
    fs::create_dir_all(dir).unwrap();
    fs::write(dir.join(file), data).unwrap();
    let manifest = serde_json::json!({
        "schema": 5,
        "id": id,
        "kind": kind,
        "version": "test",
        "license": "Apache-2.0",
        "files": [{"path": file, "sha256": sha256(data)}],
        "runtime": {
            "backend": "cpu",
            "format": "onnx",
            "precision": if kind == "asr" { "int8" } else { "fp32" },
            "max_segment_ms": 30000
        }
    });
    fs::write(
        dir.join(MANIFEST_FILE),
        serde_json::to_vec_pretty(&manifest).unwrap(),
    )
    .unwrap();
}

fn write_core_models(root: &std::path::Path) {
    write_manifest(
        &root.join("firered-vad"),
        "firered-vad",
        "vad",
        "model.onnx",
        b"vad",
    );
    write_manifest(
        &root.join("sensevoice-small"),
        "sensevoice-small",
        "asr",
        "model.int8.onnx",
        b"asr",
    );
    fs::write(
        root.join("sensevoice-small").join("tokens.txt"),
        b"<blank> 0\n",
    )
    .unwrap();
}

#[test]
fn loads_two_single_model_directories() {
    let directory = tempfile::tempdir().unwrap();
    write_core_models(directory.path());
    let models = load_models(directory.path()).unwrap();
    assert_eq!(models.vad.id, "firered-vad");
    assert_eq!(models.asr.id, "sensevoice-small");
    assert_eq!(models.asr.runtime.precision, "int8");
}

#[test]
fn rejects_old_schema_and_wrong_directory_id() {
    let directory = tempfile::tempdir().unwrap();
    write_core_models(directory.path());
    let manifest_path = directory.path().join("firered-vad").join(MANIFEST_FILE);
    let mut value: serde_json::Value =
        serde_json::from_slice(&fs::read(&manifest_path).unwrap()).unwrap();
    value["schema"] = serde_json::json!(4);
    fs::write(&manifest_path, serde_json::to_vec(&value).unwrap()).unwrap();
    assert!(load_models(directory.path()).is_err());

    let directory = tempfile::tempdir().unwrap();
    write_core_models(directory.path());
    let manifest_path = directory
        .path()
        .join("sensevoice-small")
        .join(MANIFEST_FILE);
    let mut value: serde_json::Value =
        serde_json::from_slice(&fs::read(&manifest_path).unwrap()).unwrap();
    value["id"] = serde_json::json!("other");
    fs::write(&manifest_path, serde_json::to_vec(&value).unwrap()).unwrap();
    assert!(load_models(directory.path()).is_err());
}

#[test]
fn verify_detects_modified_weights() {
    let directory = tempfile::tempdir().unwrap();
    write_core_models(directory.path());
    fs::write(directory.path().join("firered-vad/model.onnx"), b"changed").unwrap();
    assert!(load_models(directory.path()).is_err());
}

#[test]
fn ignores_legacy_size_field() {
    let directory = tempfile::tempdir().unwrap();
    write_core_models(directory.path());
    let manifest_path = directory.path().join("firered-vad").join(MANIFEST_FILE);
    let mut value: serde_json::Value =
        serde_json::from_slice(&fs::read(&manifest_path).unwrap()).unwrap();
    value["files"][0]["size"] = serde_json::json!(999);
    fs::write(&manifest_path, serde_json::to_vec(&value).unwrap()).unwrap();
    assert!(load_models(directory.path()).is_ok());
}

#[test]
fn manifest_rejects_parent_paths() {
    let directory = tempfile::tempdir().unwrap();
    let manifest: ModelManifest = serde_json::from_value(serde_json::json!({
        "schema": 5,
        "id": "firered-vad",
        "kind": "vad",
        "version": "test",
        "license": "Apache-2.0",
        "files": [{"path": "../escape.onnx", "sha256": "0".repeat(64)}],
        "runtime": {
            "backend": "cpu", "format": "onnx",
            "precision": "fp32", "max_segment_ms": 30000
        }
    }))
    .unwrap();
    assert!(manifest.verify(directory.path()).is_err());
}
