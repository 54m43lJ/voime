#!/usr/bin/env python3
"""Install the optional FireRed AED GPU package.

The runtime never calls this script. By default it downloads the official
FireRedASR v1 FP16 ONNX package and pairs it with FireRedVAD / FireRedPunc from
a support package. `--source-dir` can point to a locally exported v2 FP16
package instead. It writes a schema 3 manifest with backend.kind = worker and
optionally activates the package.
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

PACKAGE_ID = "fire-red-aed-gpu"
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


def default_cache_dir() -> Path:
    if os.name == "nt":
        base = os.environ.get("LOCALAPPDATA") or str(
            Path.home() / "AppData" / "Local"
        )
    else:
        base = os.environ.get("XDG_CACHE_HOME") or str(Path.home() / ".cache")
    return Path(base) / "voime"


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
        "请用 --support-dir 指定含 VAD / FireRedPunc 的模型包，"
        "或先用 tools/export_models.py 生成 FireRedPunc ONNX"
    )


def copy_artifact(source: Path, artifact: dict, target: Path) -> None:
    relative = Path(artifact["model"])
    destination = target / relative
    destination.parent.mkdir(parents=True, exist_ok=True)
    shutil.copy2(source / relative, destination)


def find_aed_files(
    source_dir: Path,
) -> tuple[Path, Path, Path, str, str, str]:
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
    source_dir: Path | None,
    support_dir: Path | None,
    cache_dir: Path,
    variant: str,
    activate: bool,
) -> int:
    root = model_root.expanduser().resolve()
    target = (target or root / PACKAGE_ID).expanduser().resolve()
    target.parent.mkdir(parents=True, exist_ok=True)
    cache_dir = cache_dir.expanduser().resolve()
    cache_dir.mkdir(parents=True, exist_ok=True)

    aed_dir = source_dir.expanduser().resolve() if source_dir else None
    if aed_dir is None or not any(
        (aed_dir / name).is_file()
        for name in ("encoder.fp16.onnx", "encoder.int8.onnx")
    ):
        if variant == "v2":
            fail("v2 需要 --source-dir 提供导出的 FP16 encoder / decoder")
        log("使用 FireRedASR v1 官方 FP16 包")
        aed_dir = download_v1(cache_dir)

    support, support_manifest = find_support_package(root, support_dir or aed_dir)
    (
        encoder,
        decoder,
        tokens,
        precision,
        encoder_name,
        decoder_name,
    ) = find_aed_files(aed_dir)
    found_tokens = token_count(tokens)
    if precision == "fp16":
        if found_tokens not in (TOKEN_COUNT_V1, TOKEN_COUNT_V2):
            fail(f"FP16 tokens 数量应为 7832（v1）或 8667（v2），实际 {found_tokens}")
    elif found_tokens != TOKEN_COUNT_V1:
        fail(f"int8 tokens 数量应为 7832，实际 {found_tokens}")
    token_count = found_tokens

    stage = Path(tempfile.mkdtemp(prefix=f".{PACKAGE_ID}-", dir=str(target.parent)))
    try:
        (stage / "asr").mkdir(parents=True, exist_ok=True)
        shutil.copy2(encoder, stage / "asr" / encoder_name)
        shutil.copy2(decoder, stage / "asr" / decoder_name)
        shutil.copy2(tokens, stage / "asr" / "tokens.txt")
        copy_artifact(support, support_manifest["vad"], stage)
        copy_artifact(support, support_manifest["punc"]["model"], stage)
        copy_artifact(
            support, {"model": support_manifest["punc"]["vocab"]}, stage
        )
        copy_artifact(
            support, {"model": support_manifest["punc"]["labels"]}, stage
        )

        manifest = {
            "schema": 3,
            "display_name": (
                "FireRedASR2-AED (GPU, FP16)"
                if precision == "fp16" and token_count == TOKEN_COUNT_V2
                else (
                    "FireRedASR v1 (GPU, FP16, temporary)"
                    if precision == "fp16"
                    else "FireRedASR v1 (GPU, INT8, temporary)"
                )
            ),
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
                    "model": f"asr/{encoder_name}",
                    "sha256": sha256_file(stage / "asr" / encoder_name),
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
                    "model": f"asr/{decoder_name}",
                    "sha256": sha256_file(stage / "asr" / decoder_name),
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
                        {"name": "logits", "dtype": "float32", "shape": [1, 1, token_count]},
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
                "precision": precision,
                "tokens": "asr/tokens.txt",
                "token_count": token_count,
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
    parser.add_argument("--source-dir", type=Path)
    parser.add_argument("--support-dir", type=Path)
    parser.add_argument("--cache-dir", type=Path, default=default_cache_dir())
    parser.add_argument(
        "--variant",
        choices=["v1", "v2", "auto"],
        default="v1",
        help="v1 = official FP16 package, v2 = local fp16 export, auto = source first",
    )
    parser.add_argument("--activate", action="store_true")
    args = parser.parse_args(argv)
    return install(
        model_root=args.model_root,
        target=args.model_dir,
        source_dir=args.source_dir,
        support_dir=args.support_dir,
        cache_dir=args.cache_dir,
        variant=args.variant,
        activate=args.activate,
    )


if __name__ == "__main__":
    import sys

    sys.exit(main())
