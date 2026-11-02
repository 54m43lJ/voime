#!/usr/bin/env python3
"""Install the optional FireRedPunc model directory (`models/punc`).

The source can be a directory containing model.onnx / vocab.txt / labels.json,
or an older package with a `punc` section in its manifest.json.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import shutil
import tempfile
from pathlib import Path


def log(message: str) -> None:
    print(f"[punc] {message}", flush=True)


def fail(message: str) -> None:
    raise SystemExit(f"[punc] {message}")


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


def find_punc(source: Path) -> tuple[Path, Path, Path]:
    for base in [source, source / "punc"]:
        model = base / "model.onnx"
        vocab = base / "vocab.txt"
        labels = base / "labels.json"
        if model.is_file() and vocab.is_file() and labels.is_file():
            return model, vocab, labels
    manifest = source / "manifest.json"
    if manifest.is_file():
        try:
            data = json.loads(manifest.read_text("utf-8"))
        except Exception:
            data = {}
        punc = data.get("punc")
        if isinstance(punc, dict):
            model = source / punc.get("model", {}).get("model", "")
            vocab = source / punc.get("vocab", "")
            labels = source / punc.get("labels", "")
            if model.is_file() and vocab.is_file() and labels.is_file():
                return model, vocab, labels
    fail(
        f"找不到 punc/model.onnx、vocab.txt、labels.json：{source}；"
        "请用 --source-dir 指向已导出的 FireRedPunc 目录"
    )


def manifest(model: Path, vocab: Path, labels: Path, max_tokens: int) -> dict:
    return {
        "schema": 3,
        "kind": "punc",
        "model": {
            "model": "model.onnx",
            "sha256": sha256_file(model),
            "inputs": [
                {"name": "input_ids", "dtype": "int64", "shape": [1, -1]},
                {"name": "attention_mask", "dtype": "int64", "shape": [1, -1]},
                {"name": "token_type_ids", "dtype": "int64", "shape": [1, -1]},
            ],
            "outputs": [
                {"name": "logits", "dtype": "float32", "shape": [1, -1, -1]}
            ],
        },
        "vocab": "vocab.txt",
        "labels": "labels.json",
        "max_tokens": max_tokens,
    }


def install(*, model_root: Path, source_dir: Path, max_tokens: int) -> int:
    root = model_root.expanduser().resolve()
    target = root / "punc"
    target.mkdir(parents=True, exist_ok=True)
    source = source_dir.expanduser().resolve()
    model, vocab, labels = find_punc(source)

    stage = Path(tempfile.mkdtemp(prefix=".punc-", dir=str(root)))
    try:
        shutil.copy2(model, stage / "model.onnx")
        shutil.copy2(vocab, stage / "vocab.txt")
        shutil.copy2(labels, stage / "labels.json")
        (stage / "manifest.json").write_text(
            json.dumps(
                manifest(
                    stage / "model.onnx",
                    stage / "vocab.txt",
                    stage / "labels.json",
                    max_tokens,
                ),
                ensure_ascii=False,
                indent=2,
            )
            + "\n",
            encoding="utf-8",
        )
        for child in stage.iterdir():
            destination = target / child.name
            if destination.is_dir():
                shutil.rmtree(destination)
            elif destination.exists():
                destination.unlink()
            shutil.move(str(child), str(destination))
    finally:
        shutil.rmtree(stage, ignore_errors=True)
    log(f"installed -> {target}")
    return 0


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--model-root", type=Path, default=default_model_root())
    parser.add_argument("--source-dir", type=Path, required=True)
    parser.add_argument("--max-tokens", type=int, default=400)
    args = parser.parse_args(argv)
    return install(
        model_root=args.model_root,
        source_dir=args.source_dir,
        max_tokens=args.max_tokens,
    )


if __name__ == "__main__":
    import sys

    sys.exit(main())
