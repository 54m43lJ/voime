#!/usr/bin/env python3
"""Install the optional FireRedASR2-AED FP16 GPU package.

The runtime never calls this script. The installer expects FP16 encoder /
decoder files exported from FireRedASR2S plus a FireRedPunc ONNX package. It
writes a schema 3 manifest with backend.kind = worker and optionally activates
the package.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import shutil
import tempfile
from pathlib import Path

PACKAGE_ID = "fire-red-aed-gpu"
TOKEN_COUNT = 8_667


def log(message: str) -> None:
    print(f"[fire-red-aed] {message}", flush=True)


def fail(message: str) -> None:
    raise SystemExit(f"[fire-red-aed] {message}")


def default_model_root() -> Path:
    if os.name == "nt":
        base = os.environ.get("LOCALAPPDATA") or str(
            Path.home() / "AppData" / "Local"
        )
    else:
        base = os.environ.get("XDG_DATA_HOME") or str(
            Path.home() / ".local" / "share"
        )
    return Path(base) / "voime" / "models"


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for block in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(block)
    return digest.hexdigest()


def token_count(tokens: Path) -> int:
    count = 0
    with tokens.open("r", encoding="utf-8") as handle:
        for line in handle:
            line = line.strip()
            if not line:
                continue
            _, _, id_text = line.rpartition(" ")
            try:
                count = max(count, int(id_text) + 1)
            except ValueError:
                continue
    return count


def find_support_package(
    root: Path, source_dir: Path | None
) -> tuple[Path, dict]:
    candidates: list[Path] = []
    if source_dir is not None:
        candidates.append(source_dir)
    candidates.append(root)
    if root.is_dir():
        candidates.extend(sorted(path for path in root.iterdir() if path.is_dir()))
    for candidate in candidates:
        manifest_path = candidate / "manifest.json"
        if not manifest_path.is_file():
            continue
        try:
            manifest = json.loads(manifest_path.read_text("utf-8"))
        except Exception:
            continue
        vad = manifest.get("vad")
        punc = manifest.get("punc")
        if not isinstance(vad, dict) or not isinstance(punc, dict):
            continue
        if not (candidate / vad.get("model", "")).is_file():
            continue
        if not (candidate / punc.get("model", {}).get("model", "")).is_file():
            continue
        log(f"support package {candidate}")
        return candidate, manifest
    fail(
        "找不到包含 FireRedVAD 与 FireRedPunc 的已有模型包；"
        "请用 --source-dir 指定导出的 FP16 AED / punc 目录，"
        "或先用 tools/export_models.py 生成 FireRedPunc ONNX"
    )


def copy_artifact(source: Path, artifact: dict, target: Path) -> None:
    relative = Path(artifact["model"])
    destination = target / relative
    destination.parent.mkdir(parents=True, exist_ok=True)
    shutil.copy2(source / relative, destination)


def find_fp16_files(source_dir: Path) -> tuple[Path, Path, Path]:
    for base in [source_dir, source_dir / "asr"]:
        encoder = base / "encoder.fp16.onnx"
        decoder = base / "decoder.fp16.onnx"
        tokens = base / "tokens.txt"
        if encoder.is_file() and decoder.is_file() and tokens.is_file():
            return encoder, decoder, tokens
    fail(
        "找不到 encoder.fp16.onnx / decoder.fp16.onnx / tokens.txt；"
        "请先用 FireRedASR2S 导出 FP16 ONNX 并通过 --source-dir 提供"
    )


def write_active(root: Path, package_id: str) -> None:
    root.mkdir(parents=True, exist_ok=True)
    temporary = root / f".active.tmp-{os.getpid()}"
    temporary.write_text(f"{package_id}\n", encoding="utf-8")
    os.replace(temporary, root / "active")
    log(f"active -> {package_id}")


def atomic_install(stage: Path, target: Path) -> None:
    target.parent.mkdir(parents=True, exist_ok=True)
    backup = None
    if target.exists():
        backup = target.with_name(f"{target.name}.backup-{os.getpid()}")
        if backup.exists():
            shutil.rmtree(backup) if backup.is_dir() else backup.unlink()
        os.replace(target, backup)
    try:
        os.replace(stage, target)
    except Exception:
        if backup and backup.exists() and not target.exists():
            os.replace(backup, target)
        raise
    if backup and backup.exists():
        shutil.rmtree(backup) if backup.is_dir() else backup.unlink()
    log(f"installed -> {target}")


def install(
    *,
    model_root: Path,
    target: Path | None,
    source_dir: Path,
    activate: bool,
) -> int:
    root = model_root.expanduser().resolve()
    target = (target or root / PACKAGE_ID).expanduser().resolve()
    target.parent.mkdir(parents=True, exist_ok=True)
    source_dir = source_dir.expanduser().resolve()

    support, support_manifest = find_support_package(root, source_dir)
    encoder, decoder, tokens = find_fp16_files(source_dir)

    stage = Path(tempfile.mkdtemp(prefix=f".{PACKAGE_ID}-", dir=str(target.parent)))
    try:
        (stage / "asr").mkdir(parents=True, exist_ok=True)
        shutil.copy2(encoder, stage / "asr" / "encoder.fp16.onnx")
        shutil.copy2(decoder, stage / "asr" / "decoder.fp16.onnx")
        shutil.copy2(tokens, stage / "asr" / "tokens.txt")
        copy_artifact(support, support_manifest["vad"], stage)
        copy_artifact(support, support_manifest["punc"]["model"], stage)
        copy_artifact(
            support, {"model": support_manifest["punc"]["vocab"]}, stage
        )
        copy_artifact(
            support, {"model": support_manifest["punc"]["labels"]}, stage
        )

        found_tokens = token_count(stage / "asr" / "tokens.txt")
        if found_tokens != TOKEN_COUNT:
            fail(f"tokens.txt 条目 {found_tokens}，但 FireRed AED 要求 {TOKEN_COUNT}")

        manifest = {
            "schema": 3,
            "display_name": "FireRedASR2-AED (GPU, FP16)",
            "requires": ["sense-voice-2024-int8"],
            "sample_rate": 16000,
            "fbank": {
                "num_bins": 80,
                "frame_ms": 25,
                "shift_ms": 10,
                "window_type": "povey",
            },
            "max_segment_ms": 30000,
            "vad": support_manifest["vad"],
            "backend": {
                "kind": "worker",
                "runtime": "onnx-migraphx",
                "device": "auto",
                "min_vram_mb": 6144,
                "supports_partial": False,
                "load_timeout_ms": 120000,
                "request_timeout_ms": 60000,
            },
            "asr": {
                "kind": "fire-red-aed",
                "encoder": {
                    "model": "asr/encoder.fp16.onnx",
                    "sha256": sha256_file(stage / "asr" / "encoder.fp16.onnx"),
                    "inputs": [
                        {"name": "x", "dtype": "float32", "shape": [1, -1, 80]},
                        {"name": "x_lens", "dtype": "int64", "shape": [1]},
                    ],
                    "outputs": [
                        {
                            "name": "cross_k",
                            "dtype": "float32",
                            "shape": [-1, -1, -1, -1],
                        },
                        {
                            "name": "cross_v",
                            "dtype": "float32",
                            "shape": [-1, -1, -1, -1],
                        },
                    ],
                },
                "decoder": {
                    "model": "asr/decoder.fp16.onnx",
                    "sha256": sha256_file(stage / "asr" / "decoder.fp16.onnx"),
                    "inputs": [
                        {"name": "tokens", "dtype": "int64", "shape": [1, 1]},
                        {
                            "name": "self_k",
                            "dtype": "float32",
                            "shape": [-1, -1, -1, -1, -1],
                        },
                        {
                            "name": "self_v",
                            "dtype": "float32",
                            "shape": [-1, -1, -1, -1, -1],
                        },
                        {
                            "name": "cross_k",
                            "dtype": "float32",
                            "shape": [-1, -1, -1, -1],
                        },
                        {
                            "name": "cross_v",
                            "dtype": "float32",
                            "shape": [-1, -1, -1, -1],
                        },
                        {"name": "offset", "dtype": "int64", "shape": [1]},
                    ],
                    "outputs": [
                        {"name": "logits", "dtype": "float32", "shape": [1, 1, TOKEN_COUNT]},
                        {
                            "name": "self_k",
                            "dtype": "float32",
                            "shape": [-1, -1, -1, -1, -1],
                        },
                        {
                            "name": "self_v",
                            "dtype": "float32",
                            "shape": [-1, -1, -1, -1, -1],
                        },
                    ],
                },
                "precision": "fp16",
                "tokens": "asr/tokens.txt",
                "token_count": TOKEN_COUNT,
                "options": {"punc": "external", "partial": False},
            },
            "punc": support_manifest["punc"],
        }
        (stage / "manifest.json").write_text(
            json.dumps(manifest, ensure_ascii=False, indent=2) + "\n",
            encoding="utf-8",
        )
        atomic_install(stage, target)
    except Exception:
        shutil.rmtree(stage, ignore_errors=True)
        raise

    if activate or not (root / "active").is_file():
        try:
            relative = target.relative_to(root)
        except ValueError:
            log(f"target 不在模型根目录下，未自动激活：{target}")
        else:
            if len(relative.parts) == 1:
                write_active(root, relative.parts[0])
            else:
                log(f"target 不是单层包目录，未自动激活：{target}")
    return 0


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--model-root", type=Path, default=default_model_root())
    parser.add_argument("--model-dir", type=Path)
    parser.add_argument("--source-dir", type=Path, required=True)
    parser.add_argument("--activate", action="store_true")
    args = parser.parse_args(argv)
    return install(
        model_root=args.model_root,
        target=args.model_dir,
        source_dir=args.source_dir,
        activate=args.activate,
    )


if __name__ == "__main__":
    import sys

    sys.exit(main())
