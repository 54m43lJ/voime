#!/usr/bin/env python3
"""Install a SenseVoice 2024-07-17 model package for Voime.

The runtime never calls this script. It downloads the sherpa-onnx model
archive and a FireRed VAD (or reuses VAD / FireRedPunc from an existing
package), writes the SHA-256 values into manifest.json, and optionally
activates the result.
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

PACKAGE = "sherpa-onnx-sense-voice-zh-en-ja-ko-yue-int8-2024-07-17"
URLS = [
    "https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models/"
    f"{PACKAGE}.tar.bz2",
    "https://huggingface.co/csukuangfj/"
    f"{PACKAGE}/resolve/main/{PACKAGE}.tar.bz2",
]
TOKEN_COUNT = 25_055
VAD_ONNX_URL = (
    "https://raw.githubusercontent.com/FireRedTeam/FireRedVAD/main/"
    "pretrained_models/onnx_models/fireredvad_stream_vad_with_cache.onnx"
)


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


def read_manifest(package_dir: Path) -> dict:
    try:
        return json.loads((package_dir / "manifest.json").read_text("utf-8"))
    except Exception as error:
        fail(f"无法读取 {package_dir / 'manifest.json'}：{error}")


def find_source(
    root: Path, explicit: Path | None, need_punc: bool, required: bool = True
) -> tuple[Path, dict] | None:
    candidates: list[Path] = []
    if explicit is not None:
        candidates.append(explicit)
    candidates.append(root)
    if root.is_dir():
        candidates.extend(sorted(path for path in root.iterdir() if path.is_dir()))
    for candidate in candidates:
        manifest_path = candidate / "manifest.json"
        if not manifest_path.is_file():
            continue
        manifest = read_manifest(candidate)
        vad = manifest.get("vad")
        if not isinstance(vad, dict) or not (candidate / vad.get("model", "")).is_file():
            continue
        if need_punc:
            punc = manifest.get("punc")
            if not isinstance(punc, dict):
                continue
            if not (candidate / punc.get("model", {}).get("model", "")).is_file():
                continue
        log(f"source package {candidate}")
        return candidate, manifest
    if not required:
        return None
    kind = "VAD / 标点" if need_punc else "VAD"
    fail(f"找不到包含 {kind} 的已有模型包；请先用 --source-dir 指定 FireRed 包目录")


def copy_artifact(source: Path, artifact: dict, target: Path) -> None:
    relative = Path(artifact["model"])
    destination = target / relative
    destination.parent.mkdir(parents=True, exist_ok=True)
    shutil.copy2(source / relative, destination)


def vad_artifact(path: Path) -> dict:
    return {
        "model": "vad/stream-vad.onnx",
        "sha256": sha256_file(path),
        "inputs": [
            {"name": "feat", "dtype": "float32", "shape": [1, -1, 80]},
            {
                "name": "caches_in",
                "dtype": "float32",
                "shape": [8, 1, 128, 19],
            },
        ],
        "outputs": [
            {"name": "probs", "dtype": "float32", "shape": [1, -1, 1]},
            {
                "name": "caches_out",
                "dtype": "float32",
                "shape": [8, 1, 128, -1],
            },
        ],
    }


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


def write_active(root: Path, package_id: str) -> None:
    root.mkdir(parents=True, exist_ok=True)
    temporary = root / f".active.tmp-{os.getpid()}"
    temporary.write_text(f"{package_id}\n", "utf-8")
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
    cache_dir: Path,
    source_dir: Path | None,
    variant: str,
    activate: bool,
) -> int:
    package_id = f"sense-voice-2024-{variant}"
    root = model_root.expanduser().resolve()
    target = (target or root / package_id).expanduser().resolve()
    target.parent.mkdir(parents=True, exist_ok=True)
    cache_dir = cache_dir.expanduser().resolve()
    cache_dir.mkdir(parents=True, exist_ok=True)
    source_dir = source_dir.expanduser().resolve() if source_dir else None
    need_punc = variant == "punc"
    found = find_source(root, source_dir, need_punc, required=need_punc)
    if found is None:
        vad_path = download(
            VAD_ONNX_URL,
            cache_dir / "vad" / "fireredvad_stream_vad_with_cache.onnx",
        )
        vad = vad_artifact(vad_path)
    else:
        source, source_manifest = found
        vad = source_manifest["vad"]

    archive = cache_dir / "sensevoice" / f"{PACKAGE}.tar.bz2"
    try:
        download(URLS[0], archive)
    except Exception as error:
        log(f"GitHub 下载失败（{error}），改用 Hugging Face")
        download(URLS[1], archive)
    extracted = extract_archive(archive, cache_dir / "sensevoice")

    stage = Path(
        tempfile.mkdtemp(prefix=f".{package_id}-", dir=str(target.parent))
    )
    try:
        (stage / "asr").mkdir(parents=True, exist_ok=True)
        shutil.copy2(extracted / "model.int8.onnx", stage / "asr" / "model.int8.onnx")
        shutil.copy2(extracted / "tokens.txt", stage / "asr" / "tokens.txt")
        for pattern in ("LICENSE*", "README*", "*.md"):
            for file in extracted.glob(pattern):
                if file.is_file():
                    shutil.copy2(file, stage / file.name)

        if found is None:
            destination = stage / vad["model"]
            destination.parent.mkdir(parents=True, exist_ok=True)
            shutil.copy2(vad_path, destination)
        else:
            copy_artifact(source, vad, stage)
        if need_punc:
            copy_artifact(source, source_manifest["punc"]["model"], stage)
            copy_artifact(
                source,
                {"model": source_manifest["punc"]["vocab"]},
                stage,
            )
            copy_artifact(
                source,
                {"model": source_manifest["punc"]["labels"]},
                stage,
            )

        found_tokens = token_count(stage / "asr" / "tokens.txt")
        if found_tokens != TOKEN_COUNT:
            log(f"tokens.txt 条目 {found_tokens}，清单仍按 {TOKEN_COUNT} 写入")
        asr = {
            "kind": "sense-voice",
            "model": {
                "model": "asr/model.int8.onnx",
                "sha256": sha256_file(stage / "asr" / "model.int8.onnx"),
                "inputs": [
                    {"name": "x", "dtype": "float32", "shape": [1, -1, 560]},
                    {"name": "x_length", "dtype": "int32", "shape": [1]},
                    {"name": "language", "dtype": "int32", "shape": [1]},
                    {"name": "text_norm", "dtype": "int32", "shape": [1]},
                ],
                "outputs": [
                    {"name": "logits", "dtype": "float32", "shape": [1, -1, TOKEN_COUNT]}
                ],
            },
            "tokens": "asr/tokens.txt",
            "token_count": TOKEN_COUNT,
            "options": {
                "language": "auto",
                "use_itn": not need_punc,
                "punc": "external" if need_punc else "builtin",
                "partial": True,
            },
        }
        manifest = {
            "schema": 2,
            "display_name": "SenseVoice 2024 ITN"
            if not need_punc
            else "SenseVoice 2024 + FireRedPunc",
            "sample_rate": 16000,
            "fbank": {
                "num_bins": 80,
                "frame_ms": 25,
                "shift_ms": 10,
                "window_type": "hamming",
            },
            "max_segment_ms": 20000,
            "vad": vad,
            "asr": asr,
        }
        if need_punc:
            manifest["punc"] = source_manifest["punc"]
            # Normalize auxiliary paths copied from the source manifest.
            manifest["punc"]["vocab"] = source_manifest["punc"]["vocab"]
            manifest["punc"]["labels"] = source_manifest["punc"]["labels"]

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
            relative = target.resolve().relative_to(root)
        except ValueError:
            log(f"target 不在模型根目录下，未自动激活：{target}")
        else:
            if len(relative.parts) == 1:
                write_active(root, relative.parts[0])
            else:
                log(f"target 不是单层包目录，未自动激活：{target}")
    log("完成。切换后执行 voime ctl restart-backend 或重启 voimed。")
    return 0


def add_arguments(parser: argparse.ArgumentParser) -> None:
    parser.add_argument("--asr", default="sense-voice")
    parser.add_argument("--variant", choices=["itn", "punc"], default="itn")
    parser.add_argument("--model-root", type=Path, default=default_model_root())
    parser.add_argument("--model-dir", type=Path)
    parser.add_argument("--cache-dir", type=Path, default=default_cache_dir())
    parser.add_argument("--source-dir", type=Path)
    parser.add_argument("--activate", action="store_true")


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    add_arguments(parser)
    args = parser.parse_args(argv)
    return install(
        model_root=args.model_root,
        target=args.model_dir,
        cache_dir=args.cache_dir,
        source_dir=args.source_dir,
        variant=args.variant,
        activate=args.activate,
    )


if __name__ == "__main__":
    sys.exit(main())
