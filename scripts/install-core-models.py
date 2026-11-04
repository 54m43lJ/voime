#!/usr/bin/env python3
"""Install the two core models as schema 5, one-model-per-directory layouts.

Sources, in order: an explicit --source-dir, the local cache, or the fixed
upstream download URLs. Each target directory is staged and then replaced, so a
failed download never leaves a half-written model directory.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import shutil
import sys
import tarfile
import tempfile
import urllib.request
from pathlib import Path

VAD_ID = "firered-vad"
VAD_VERSION = "2026-02-12"
VAD_URLS = [
    "https://raw.githubusercontent.com/FireRedTeam/FireRedVAD/main/"
    "pretrained_models/onnx_models/fireredvad_stream_vad_with_cache.onnx",
]
VAD_CACHE_NAME = "fireredvad_stream_vad_with_cache.onnx"

SENSEVOICE_ID = "sensevoice-small"
SENSEVOICE_PACKAGE = "sherpa-onnx-sense-voice-zh-en-ja-ko-yue-int8-2024-07-17"
SENSEVOICE_VERSION = "2024-07-17"
SENSEVOICE_URLS = [
    "https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models/"
    f"{SENSEVOICE_PACKAGE}.tar.bz2",
    "https://huggingface.co/csukuangfj/"
    f"{SENSEVOICE_PACKAGE}/resolve/main/{SENSEVOICE_PACKAGE}.tar.bz2",
]

SCHEMA = 5
MAX_SEGMENT_MS = 30_000


def log(message: str) -> None:
    print(f"[core-models] {message}", flush=True)


def fail(message: str) -> "NoReturn":
    raise SystemExit(f"[core-models] {message}")


def default_model_root() -> Path:
    if os.name == "nt":
        base = os.environ.get("LOCALAPPDATA") or str(Path.home() / "AppData" / "Local")
    else:
        base = os.environ.get("XDG_DATA_HOME") or str(Path.home() / ".local" / "share")
    return Path(base) / "voime" / "models"


def default_cache_dir() -> Path:
    if os.name == "nt":
        base = os.environ.get("LOCALAPPDATA") or str(Path.home() / "AppData" / "Local")
    else:
        base = os.environ.get("XDG_CACHE_HOME") or str(Path.home() / ".cache")
    return Path(base) / "voime"


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for block in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(block)
    return digest.hexdigest()


def download(urls: list[str], destination: Path, allow_download: bool) -> Path:
    if destination.is_file() and destination.stat().st_size > 0:
        log(f"cache hit {destination}")
        return destination
    if not allow_download:
        fail(f"缺少缓存文件且已禁用下载：{destination}")
    destination.parent.mkdir(parents=True, exist_ok=True)
    last_error: Exception | None = None
    for url in urls:
        temporary = destination.with_suffix(destination.suffix + ".part")
        log(f"download {url}")
        try:
            with urllib.request.urlopen(url, timeout=120) as response, temporary.open(
                "wb"
            ) as handle:
                shutil.copyfileobj(response, handle)
            temporary.replace(destination)
            return destination
        except Exception as error:  # try the next mirror
            last_error = error
            temporary.unlink(missing_ok=True)
            log(f"download failed: {error}")
    fail(f"模型下载失败：{last_error}")


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
    for model in destination.glob("*/model.int8.onnx"):
        if (model.parent / "tokens.txt").is_file():
            return model.parent
    fail(f"压缩包中找不到 SenseVoice 模型：{destination}")


def find_file(root: Path, names: tuple[str, ...]) -> Path | None:
    for name in names:
        direct = root / name
        if direct.is_file():
            return direct
    for name in names:
        matches = list(root.rglob(name))
        if matches:
            return matches[0]
    return None


def manifest(
    model_id: str,
    kind: str,
    version: str,
    precision: str,
    files: list[tuple[str, Path]],
) -> dict:
    entries = []
    for relative, source in files:
        entries.append(
            {
                "path": relative,
                "sha256": sha256_file(source),
                "size": source.stat().st_size,
            }
        )
    return {
        "schema": SCHEMA,
        "id": model_id,
        "kind": kind,
        "version": version,
        "license": "Apache-2.0",
        "files": entries,
        "runtime": {
            "backend": "cpu",
            "format": "onnx",
            "precision": precision,
            "max_segment_ms": MAX_SEGMENT_MS,
        },
    }


def valid_target(target: Path, model_id: str) -> bool:
    manifest_path = target / "manifest.json"
    if not manifest_path.is_file():
        return False
    try:
        value = json.loads(manifest_path.read_text(encoding="utf-8"))
    except (OSError, ValueError):
        return False
    if value.get("schema") != SCHEMA or value.get("id") != model_id:
        return False
    for file in value.get("files", []):
        path = target / file.get("path", "")
        if not path.is_file() or path.stat().st_size != file.get("size"):
            return False
        if sha256_file(path) != file.get("sha256"):
            return False
    return True


def stage(
    root: Path,
    model_id: str,
    kind: str,
    version: str,
    precision: str,
    files: list[tuple[str, Path]],
) -> Path:
    root.mkdir(parents=True, exist_ok=True)
    stage_dir = Path(tempfile.mkdtemp(prefix=f".{model_id}-", dir=root))
    try:
        for relative, source in files:
            destination = stage_dir / relative
            destination.parent.mkdir(parents=True, exist_ok=True)
            shutil.copy2(source, destination)
        (stage_dir / "manifest.json").write_text(
            json.dumps(
                manifest(model_id, kind, version, precision, files),
                ensure_ascii=False,
                indent=2,
            )
            + "\n",
            encoding="utf-8",
        )
    except Exception:
        shutil.rmtree(stage_dir, ignore_errors=True)
        raise
    return stage_dir


def replace_target(stage_dir: Path, target: Path) -> None:
    if target.exists():
        shutil.rmtree(target)
    stage_dir.replace(target)


def install_vad(
    model_root: Path,
    cache_dir: Path,
    source_dir: Path | None,
    allow_download: bool,
    force: bool,
) -> None:
    target = model_root / VAD_ID
    if not force and valid_target(target, VAD_ID):
        log(f"already installed {target}")
        return
    source = None
    if source_dir is not None:
        source = find_file(
            source_dir / VAD_ID,
            (
                "model.onnx",
                VAD_CACHE_NAME,
                "fireredvad_stream_vad_with_cache.onnx",
                "stream-vad.onnx",
            ),
        ) or find_file(
            source_dir,
            ("model.onnx", VAD_CACHE_NAME, "stream-vad.onnx"),
        )
    if source is None:
        source = download(
            VAD_URLS,
            cache_dir / "vad" / VAD_CACHE_NAME,
            allow_download,
        )
    stage_dir = stage(
        model_root,
        VAD_ID,
        "vad",
        VAD_VERSION,
        "fp32",
        [("model.onnx", source)],
    )
    replace_target(stage_dir, target)
    log(f"installed -> {target}")


def install_sensevoice(
    model_root: Path,
    cache_dir: Path,
    source_dir: Path | None,
    allow_download: bool,
    force: bool,
) -> None:
    target = model_root / SENSEVOICE_ID
    if not force and valid_target(target, SENSEVOICE_ID):
        log(f"already installed {target}")
        return
    source = None
    if source_dir is not None:
        source = source_dir / SENSEVOICE_ID
        if not (source / "model.int8.onnx").is_file():
            source = source_dir
        if not (source / "model.int8.onnx").is_file():
            found = find_file(source_dir, ("model.int8.onnx",))
            source = found.parent if found else None
    if source is not None and (source / "model.int8.onnx").is_file():
        model = source / "model.int8.onnx"
        tokens = source / "tokens.txt"
    else:
        cached = cache_dir / "sensevoice" / SENSEVOICE_PACKAGE
        if not (cached / "model.int8.onnx").is_file():
            archive = download(
                SENSEVOICE_URLS,
                cache_dir / "sensevoice" / f"{SENSEVOICE_PACKAGE}.tar.bz2",
                allow_download,
            )
            cached = extract_archive(archive, cache_dir / "sensevoice")
        model = cached / "model.int8.onnx"
        tokens = cached / "tokens.txt"
    if not tokens.is_file():
        fail(f"缺少 SenseVoice tokens.txt：{tokens}")
    stage_dir = stage(
        model_root,
        SENSEVOICE_ID,
        "asr",
        SENSEVOICE_VERSION,
        "int8",
        [
            ("model.int8.onnx", model),
            ("tokens.txt", tokens),
        ],
    )
    replace_target(stage_dir, target)
    log(f"installed -> {target}")


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--model-root", type=Path, default=default_model_root())
    parser.add_argument("--cache-dir", type=Path, default=default_cache_dir())
    parser.add_argument("--source-dir", type=Path)
    parser.add_argument("--no-download", action="store_true")
    parser.add_argument("--force", action="store_true")
    args = parser.parse_args(argv)
    model_root = args.model_root.expanduser().resolve()
    cache_dir = args.cache_dir.expanduser().resolve()
    source_dir = args.source_dir.expanduser().resolve() if args.source_dir else None
    install_vad(
        model_root,
        cache_dir,
        source_dir,
        not args.no_download,
        args.force,
    )
    install_sensevoice(
        model_root,
        cache_dir,
        source_dir,
        not args.no_download,
        args.force,
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
