#!/usr/bin/env python3
"""Install the optional FireRed AED model directory (`models/fire-red-aed`).

By default this downloads the official FireRedASR v1 FP16 ONNX package.
`--source-dir` can point to a locally exported v2 FP16 package instead.
"""

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

PACKAGE_ID = "fire-red-aed"
TOKEN_COUNT_V2 = 8_667
TOKEN_COUNT_V1 = 7_832
V1_PACKAGE = "sherpa-onnx-fire-red-asr-large-zh_en-fp16-2025-02-16"
V1_URL = (
    "https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models/"
    f"{V1_PACKAGE}.tar.bz2"
)
V1_HF_BASE = f"https://huggingface.co/csukuangfj/{V1_PACKAGE}/resolve/main"


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
    root = destination / V1_PACKAGE
    if not root.is_dir():
        matches = list(destination.glob("*/encoder.fp16.onnx"))
        if matches:
            root = matches[0].parent
    return root


def download_v1(cache_dir: Path) -> Path:
    root = cache_dir / "aed-v1" / V1_PACKAGE
    if (root / "encoder.fp16.onnx").is_file() and (
        root / "decoder.fp16.onnx"
    ).is_file():
        log(f"cache hit {root}")
        return root
    root.mkdir(parents=True, exist_ok=True)
    archive = cache_dir / "aed-v1" / f"{V1_PACKAGE}.tar.bz2"
    try:
        download(V1_URL, archive)
        extracted = extract_archive(archive, cache_dir / "aed-v1")
        if (extracted / "encoder.fp16.onnx").is_file():
            return extracted
        log(f"压缩包中找不到 encoder.fp16.onnx：{extracted}，改用 Hugging Face 文件")
    except Exception as error:
        log(f"GitHub 下载失败（{error}），改用 Hugging Face 文件")
    for name in ("encoder.fp16.onnx", "decoder.fp16.onnx", "tokens.txt"):
        download(f"{V1_HF_BASE}/{name}", root / name)
    if not (root / "encoder.fp16.onnx").is_file():
        fail(f"无法准备 FireRedASR v1 FP16 包：{root}")
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


def find_aed_files(source_dir: Path) -> tuple[Path, Path, Path, str, str, str]:
    for base in [source_dir, source_dir / "asr"]:
        tokens = base / "tokens.txt"
        if not tokens.is_file():
            continue
        for suffix, precision in [("fp16", "fp16"), ("int8", "int8")]:
            encoder = base / f"encoder.{suffix}.onnx"
            decoder = base / f"decoder.{suffix}.onnx"
            if encoder.is_file() and decoder.is_file():
                return (
                    encoder,
                    decoder,
                    tokens,
                    precision,
                    f"encoder.{suffix}.onnx",
                    f"decoder.{suffix}.onnx",
                )
    fail(
        "找不到 encoder.fp16.onnx / encoder.int8.onnx 与 decoder 对应文件；"
        "v2 需先导出 FP16 ONNX，v1 可下载官方 FP16 包"
    )


def manifest(
    encoder: Path,
    decoder: Path,
    tokens: Path,
    precision: str,
    count: int,
) -> dict:
    return {
        "schema": 3,
        "kind": "fire-red-aed",
        "fbank": {
            "num_bins": 80,
            "frame_ms": 25,
            "shift_ms": 10,
            "window_type": "povey",
        },
        "max_segment_ms": 30000,
        "encoder": {
            "model": encoder.name,
            "sha256": sha256_file(encoder),
            "inputs": [
                {"name": "x", "dtype": "float32", "shape": [1, -1, 80]},
                {"name": "x_lens", "dtype": "int64", "shape": [1]},
            ],
            "outputs": [
                {"name": "cross_k", "dtype": "float32", "shape": [-1, -1, -1, -1]},
                {"name": "cross_v", "dtype": "float32", "shape": [-1, -1, -1, -1]},
            ],
        },
        "decoder": {
            "model": decoder.name,
            "sha256": sha256_file(decoder),
            "inputs": [
                {"name": "tokens", "dtype": "int64", "shape": [1, 1]},
                {"name": "self_k", "dtype": "float32", "shape": [-1, -1, -1, -1, -1]},
                {"name": "self_v", "dtype": "float32", "shape": [-1, -1, -1, -1, -1]},
                {"name": "cross_k", "dtype": "float32", "shape": [-1, -1, -1, -1]},
                {"name": "cross_v", "dtype": "float32", "shape": [-1, -1, -1, -1]},
                {"name": "offset", "dtype": "int64", "shape": [1]},
            ],
            "outputs": [
                {"name": "logits", "dtype": "float32", "shape": [1, 1, count]},
                {"name": "self_k", "dtype": "float32", "shape": [-1, -1, -1, -1, -1]},
                {"name": "self_v", "dtype": "float32", "shape": [-1, -1, -1, -1, -1]},
            ],
        },
        "precision": precision,
        "tokens": "tokens.txt",
        "token_count": count,
    }


def install(
    *,
    model_root: Path,
    cache_dir: Path,
    source_dir: Path | None,
    variant: str,
) -> int:
    root = model_root.expanduser().resolve()
    target = root / PACKAGE_ID
    target.mkdir(parents=True, exist_ok=True)
    cache_dir = cache_dir.expanduser().resolve()

    aed_dir = source_dir.expanduser().resolve() if source_dir else None
    if aed_dir is None or not any(
        (aed_dir / name).is_file()
        for name in ("encoder.fp16.onnx", "encoder.int8.onnx")
    ):
        if variant == "v2":
            fail("v2 需要 --source-dir 提供导出的 FP16 encoder / decoder")
        log("使用 FireRedASR v1 官方 FP16 包")
        aed_dir = download_v1(cache_dir)

    encoder, decoder, tokens, precision, encoder_name, decoder_name = find_aed_files(
        aed_dir
    )
    count = token_count(tokens)
    if precision == "fp16":
        if count not in (TOKEN_COUNT_V1, TOKEN_COUNT_V2):
            fail(f"FP16 tokens 数量应为 7832（v1）或 8667（v2），实际 {count}")
    elif count != TOKEN_COUNT_V1:
        fail(f"int8 tokens 数量应为 7832，实际 {count}")

    stage = Path(tempfile.mkdtemp(prefix=f".{PACKAGE_ID}-", dir=str(root)))
    try:
        shutil.copy2(encoder, stage / encoder_name)
        shutil.copy2(decoder, stage / decoder_name)
        shutil.copy2(tokens, stage / "tokens.txt")
        (stage / "manifest.json").write_text(
            json.dumps(
                manifest(
                    stage / encoder_name,
                    stage / decoder_name,
                    stage / "tokens.txt",
                    precision,
                    count,
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
    parser.add_argument("--source-dir", type=Path)
    parser.add_argument(
        "--variant",
        choices=["v1", "v2", "auto"],
        default="v1",
    )
    args = parser.parse_args(argv)
    return install(
        model_root=args.model_root,
        cache_dir=args.cache_dir,
        source_dir=args.source_dir,
        variant=args.variant,
    )


if __name__ == "__main__":
    import sys

    sys.exit(main())
