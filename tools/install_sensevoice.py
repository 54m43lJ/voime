#!/usr/bin/env python3
"""Install the SenseVoice model into the default resource pack (`models/default/sense-voice`)."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import shutil
import tarfile
import tempfile
import urllib.request
from pathlib import Path

PACKAGE = "sherpa-onnx-sense-voice-zh-en-ja-ko-yue-int8-2024-07-17"
URLS = [
    "https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models/"
    f"{PACKAGE}.tar.bz2",
    "https://huggingface.co/csukuangfj/"
    f"{PACKAGE}/resolve/main/{PACKAGE}.tar.bz2",
]
TOKEN_COUNT = 25_055
PACKAGE_ID = "sense-voice"


def log(message: str) -> None:
    print(f"[sensevoice] {message}", flush=True)


def fail(message: str) -> None:
    raise SystemExit(f"[sensevoice] {message}")


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


def default_cache_dir() -> Path:
    if os.name == "nt":
        base = os.environ.get("LOCALAPPDATA") or str(
            Path.home() / "AppData" / "Local"
        )
    else:
        base = os.environ.get("XDG_CACHE_HOME") or str(Path.home() / ".cache")
    return Path(base) / "voime"


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for block in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(block)
    return digest.hexdigest()


def download(url: str, destination: Path) -> Path:
    destination.parent.mkdir(parents=True, exist_ok=True)
    if destination.is_file() and destination.stat().st_size > 0:
        log(f"cache hit {destination}")
        return destination
    temporary = destination.with_suffix(destination.suffix + ".part")
    log(f"download {url}")
    with urllib.request.urlopen(url, timeout=120) as response, temporary.open(
        "wb"
    ) as handle:
        shutil.copyfileobj(response, handle)
    temporary.replace(destination)
    return destination


def extract_archive(archive: Path, destination: Path) -> Path:
    destination.mkdir(parents=True, exist_ok=True)
    with tarfile.open(archive, "r:*") as tar:
        base = destination.resolve()
        for member in tar.getmembers():
            target = (base / member.name).resolve()
            if base not in target.parents and target != base:
                fail(f"压缩包包含越界路径：{member.name}")
        try:
            tar.extractall(destination, filter="data")
        except TypeError:
            tar.extractall(destination)
    root = destination / PACKAGE
    if not (root / "model.int8.onnx").is_file():
        matches = list(destination.glob("*/model.int8.onnx"))
        if not matches:
            fail(f"压缩包中找不到 model.int8.onnx：{destination}")
        root = matches[0].parent
    if not (root / "tokens.txt").is_file():
        fail(f"压缩包中找不到 tokens.txt：{root}")
    return root


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


def manifest(model: Path, tokens: Path, count: int) -> dict:
    return {
        "schema": 3,
        "kind": "sense-voice",
        "fbank": {
            "num_bins": 80,
            "frame_ms": 25,
            "shift_ms": 10,
            "window_type": "hamming",
        },
        "max_segment_ms": 20000,
        "model": {
            "model": "model.int8.onnx",
            "sha256": sha256_file(model),
            "inputs": [
                {"name": "x", "dtype": "float32", "shape": [1, -1, 560]},
                {"name": "x_length", "dtype": "int32", "shape": [1]},
                {"name": "language", "dtype": "int32", "shape": [1]},
                {"name": "text_norm", "dtype": "int32", "shape": [1]},
            ],
            "outputs": [
                {"name": "logits", "dtype": "float32", "shape": [1, -1, count]}
            ],
        },
        "tokens": "tokens.txt",
        "token_count": count,
        "options": {
            "language": "auto",
            "use_itn": True,
            "partial": True,
        },
    }


def install(*, model_root: Path, cache_dir: Path) -> int:
    root = model_root.expanduser().resolve()
    target = root / "default" / PACKAGE_ID
    target.mkdir(parents=True, exist_ok=True)
    cache_dir = cache_dir.expanduser().resolve()

    archive = cache_dir / "sensevoice" / f"{PACKAGE}.tar.bz2"
    try:
        download(URLS[0], archive)
    except Exception as error:
        log(f"GitHub 下载失败（{error}），改用 Hugging Face")
        download(URLS[1], archive)
    extracted = extract_archive(archive, cache_dir / "sensevoice")

    stage = Path(tempfile.mkdtemp(prefix=f".{PACKAGE_ID}-", dir=str(root)))
    try:
        shutil.copy2(extracted / "model.int8.onnx", stage / "model.int8.onnx")
        shutil.copy2(extracted / "tokens.txt", stage / "tokens.txt")
        for pattern in ("LICENSE*", "README*", "*.md"):
            for file in extracted.glob(pattern):
                if file.is_file():
                    shutil.copy2(file, stage / file.name)
        found = token_count(stage / "tokens.txt")
        if found != TOKEN_COUNT:
            fail(f"tokens.txt 条目 {found}，但 SenseVoice 要求 {TOKEN_COUNT}")
        (stage / "manifest.json").write_text(
            json.dumps(
                manifest(
                    stage / "model.int8.onnx",
                    stage / "tokens.txt",
                    found,
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
    parser.add_argument("--cache-dir", type=Path, default=default_cache_dir())
    args = parser.parse_args(argv)
    return install(model_root=args.model_root, cache_dir=args.cache_dir)


if __name__ == "__main__":
    import sys

    sys.exit(main())
