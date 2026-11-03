#!/usr/bin/env python3
"""Install the FireRedVAD model into the default resource pack (`models/default/vad`)."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import shutil
import urllib.request
from pathlib import Path

VAD_URL = (
    "https://raw.githubusercontent.com/FireRedTeam/FireRedVAD/main/"
    "pretrained_models/onnx_models/fireredvad_stream_vad_with_cache.onnx"
)


def log(message: str) -> None:
    print(f"[vad] {message}", flush=True)


def fail(message: str) -> None:
    raise SystemExit(f"[vad] {message}")


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


def manifest(model: Path) -> dict:
    return {
        "schema": 3,
        "kind": "vad",
        "fbank": {
            "num_bins": 80,
            "frame_ms": 25,
            "shift_ms": 10,
            "window_type": "hamming",
        },
        "model": {
            "model": "stream-vad.onnx",
            "sha256": sha256_file(model),
            "inputs": [
                {"name": "feat", "dtype": "float32", "shape": [1, -1, 80]},
                {"name": "caches_in", "dtype": "float32", "shape": [8, 1, 128, 19]},
            ],
            "outputs": [
                {"name": "probs", "dtype": "float32", "shape": [1, -1, 1]},
                {"name": "caches_out", "dtype": "float32", "shape": [8, 1, 128, -1]},
            ],
        },
    }


def install(
    *,
    model_root: Path,
    cache_dir: Path,
    source_dir: Path | None = None,
) -> int:
    root = model_root.expanduser().resolve()
    target = root / "default" / "vad"
    cache_dir = cache_dir.expanduser().resolve()
    target.mkdir(parents=True, exist_ok=True)

    source = None
    if source_dir is not None:
        for candidate in [
            source_dir.expanduser().resolve() / "vad" / "stream-vad.onnx",
            source_dir.expanduser().resolve() / "stream-vad.onnx",
        ]:
            if candidate.is_file():
                source = candidate
                break
    if source is None:
        source = download(
            VAD_URL,
            cache_dir / "vad" / "fireredvad_stream_vad_with_cache.onnx",
        )

    destination = target / "stream-vad.onnx"
    shutil.copy2(source, destination)
    (target / "manifest.json").write_text(
        json.dumps(manifest(destination), ensure_ascii=False, indent=2) + "\n",
        encoding="utf-8",
    )
    log(f"installed -> {target}")
    return 0


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--model-root", type=Path, default=default_model_root())
    parser.add_argument("--cache-dir", type=Path, default=default_cache_dir())
    parser.add_argument("--source-dir", type=Path)
    args = parser.parse_args(argv)
    return install(
        model_root=args.model_root,
        cache_dir=args.cache_dir,
        source_dir=args.source_dir,
    )


if __name__ == "__main__":
    import sys

    sys.exit(main())
