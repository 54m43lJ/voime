#!/usr/bin/env python3
"""Download the FireRedASR2-CTC package and build the Voime model directory.

The runtime never calls this script. `scripts/install_models.sh` creates an
isolated virtual environment with the installation-only dependencies.
"""

from __future__ import annotations

import argparse
import os
import shutil
import sys
import tarfile
import tempfile
import urllib.request
from pathlib import Path

CTC_VERSION = "2026-02-25"
CTC_PACKAGE = f"sherpa-onnx-fire-red-asr2-ctc-zh_en-int8-{CTC_VERSION}"
CTC_URL = (
    "https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models/"
    f"{CTC_PACKAGE}.tar.bz2"
)
CTC_HF_URL = (
    "https://huggingface.co/csukuangfj2/"
    f"{CTC_PACKAGE}/resolve/main/{CTC_PACKAGE}.tar.bz2"
)
PUNC_REPO = "FireRedTeam/FireRedPunc"
ASR2S_TARBALL = "https://github.com/FireRedTeam/FireRedASR2S/archive/refs/heads/main.tar.gz"
VAD_ONNX_URL = (
    "https://raw.githubusercontent.com/FireRedTeam/FireRedVAD/main/"
    "pretrained_models/onnx_models/fireredvad_stream_vad_with_cache.onnx"
)
VAD_CMVN_URL = (
    "https://raw.githubusercontent.com/FireRedTeam/FireRedVAD/main/"
    "pretrained_models/onnx_models/cmvn.ark"
)


def log(message: str) -> None:
    print(f"[install] {message}", flush=True)


def fail(message: str) -> None:
    raise SystemExit(f"[install] {message}")


def default_cache_dir() -> Path:
    if os.name == "nt":
        base = os.environ.get("LOCALAPPDATA") or str(
            Path.home() / "AppData" / "Local"
        )
        return Path(base) / "voime" / "cache"
    base = os.environ.get("XDG_CACHE_HOME") or str(Path.home() / ".cache")
    return Path(base) / "voime"


def default_model_dir() -> Path:
    if os.name == "nt":
        base = os.environ.get("LOCALAPPDATA") or str(
            Path.home() / "AppData" / "Local"
        )
        return Path(base) / "voime" / "models"
    base = os.environ.get("XDG_DATA_HOME") or str(
        Path.home() / ".local" / "share"
    )
    return Path(base) / "voime" / "models"


def check_dependencies() -> None:
    missing = []
    for module in (
        "torch",
        "onnx",
        "onnxruntime",
        "numpy",
        "huggingface_hub",
        "transformers",
        "sentencepiece",
        "kaldiio",
    ):
        try:
            __import__(module)
        except ImportError:
            missing.append(module)
    if missing:
        fail(
            "安装虚拟环境缺少依赖："
            + ", ".join(missing)
            + "；请通过 scripts/install_models.sh 运行"
        )


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


def extract_archive(archive: Path, destination: Path) -> None:
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


def download_ctc(cache_dir: Path) -> Path:
    root = cache_dir / "ctc" / CTC_PACKAGE
    if (root / "model.int8.onnx").is_file() and (root / "tokens.txt").is_file():
        log(f"cache hit {root}")
        return root
    archive = cache_dir / "ctc" / f"{CTC_PACKAGE}.tar.bz2"
    try:
        download(CTC_URL, archive)
    except Exception as error:
        log(f"GitHub 下载失败（{error}），改用 Hugging Face")
        download(CTC_HF_URL, archive)
    shutil.rmtree(root, ignore_errors=True)
    extract_archive(archive, root.parent)
    if not (root / "model.int8.onnx").is_file():
        matches = list(root.parent.glob("*/model.int8.onnx"))
        if not matches:
            fail(f"CTC 包中找不到 model.int8.onnx：{root.parent}")
        root = matches[0].parent
    return root


def snapshot_punc(cache_dir: Path) -> Path:
    destination = cache_dir / "weights" / "FireRedPunc"
    marker = destination / ".voime-complete"
    if marker.is_file():
        log(f"cache hit {destination}")
        return destination
    from huggingface_hub import snapshot_download

    log(f"download {PUNC_REPO} -> {destination}")
    destination.mkdir(parents=True, exist_ok=True)
    snapshot_download(
        repo_id=PUNC_REPO,
        local_dir=str(destination),
        max_workers=4,
        allow_patterns=[
            "model.pth.tar",
            "config.yaml",
            "out_dict",
            "chinese-bert-wwm-ext_vocab.txt",
            "chinese-lert-base/config.json",
            "chinese-lert-base/tokenizer.json",
            "chinese-lert-base/tokenizer_config.json",
            "chinese-lert-base/vocab.txt",
            "chinese-lert-base/added_tokens.json",
            "chinese-lert-base/special_tokens_map.json",
            "chinese-lert-base/pytorch_model.bin",
        ],
    )
    marker.write_text("ok\n", encoding="utf-8")
    return destination


def locate_vad(source_root: Path | None, cache_dir: Path) -> tuple[Path, Path]:
    if source_root:
        for candidate in [
            source_root / "FireRedVAD" / "pretrained_models" / "onnx_models",
            source_root / "vad",
        ]:
            onnx_path = candidate / "fireredvad_stream_vad_with_cache.onnx"
            cmvn_path = candidate / "cmvn.ark"
            if onnx_path.is_file() and cmvn_path.is_file():
                return onnx_path, cmvn_path
    vad_dir = cache_dir / "vad"
    return (
        download(VAD_ONNX_URL, vad_dir / "fireredvad_stream_vad_with_cache.onnx"),
        download(VAD_CMVN_URL, vad_dir / "cmvn.ark"),
    )


def extract_code(cache_dir: Path) -> Path:
    root = cache_dir / "code" / "FireRedASR2S-main"
    if (root / "fireredasr2s").is_dir():
        return root
    archive = cache_dir / "code" / "FireRedASR2S-main.tar.gz"
    download(ASR2S_TARBALL, archive)
    destination = cache_dir / "code"
    extract_archive(archive, destination)
    if not (root / "fireredasr2s").is_dir():
        fail(f"FireRedASR2S 源码解压失败：{root}")
    return root


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


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--model-dir", type=Path, default=default_model_dir())
    parser.add_argument("--cache-dir", type=Path, default=default_cache_dir())
    parser.add_argument(
        "--source-dir",
        type=Path,
        help="包含 FireRedPunc/ 与 FireRedVAD/ 的本地目录",
    )
    parser.add_argument("--code-dir", type=Path, help="FireRedASR2S 源码目录")
    parser.add_argument("--no-quantize", action="store_true")
    parser.add_argument("--keep-stage", action="store_true")
    args = parser.parse_args()

    check_dependencies()
    cache_dir = args.cache_dir.resolve()
    cache_dir.mkdir(parents=True, exist_ok=True)

    source_root = args.source_dir.resolve() if args.source_dir else None
    if source_root:
        punc_source = source_root / "FireRedPunc"
        if not punc_source.is_dir():
            fail(f"缺少标点权重目录：{punc_source}")
    else:
        punc_source = snapshot_punc(cache_dir)

    code_dir = args.code_dir.resolve() if args.code_dir else None
    if code_dir is None:
        if source_root and (source_root / "FireRedASR2S").is_dir():
            code_dir = source_root / "FireRedASR2S"
        else:
            code_dir = extract_code(cache_dir)
    if not (code_dir / "fireredasr2s").is_dir():
        fail(f"找不到 fireredasr2s 源码：{code_dir}")

    ctc_dir = download_ctc(cache_dir)
    vad_onnx, vad_cmvn = locate_vad(source_root, cache_dir)
    target = args.model_dir.expanduser().resolve()
    target.parent.mkdir(parents=True, exist_ok=True)

    stage = Path(tempfile.mkdtemp(prefix=".voime-models-", dir=str(target.parent)))
    try:
        (stage / "asr").mkdir(parents=True, exist_ok=True)
        shutil.copy2(ctc_dir / "model.int8.onnx", stage / "asr" / "model.int8.onnx")
        shutil.copy2(ctc_dir / "tokens.txt", stage / "asr" / "tokens.txt")

        sys.path.insert(0, str(Path(__file__).resolve().parent))
        sys.path.insert(0, str(code_dir))
        import export_models
        import verify_models

        export_models.export_all(
            punc_source=punc_source,
            vad_onnx=vad_onnx,
            vad_cmvn=vad_cmvn,
            code_dir=code_dir,
            out_dir=stage,
            quantize=not args.no_quantize,
        )
        verify_models.verify_package(stage, write_manifest=True)
        atomic_install(stage, target)
    except Exception:
        if not args.keep_stage and stage.exists():
            shutil.rmtree(stage, ignore_errors=True)
        raise
    finally:
        if args.keep_stage and stage.exists():
            log(f"stage kept at {stage}")

    log(f"model package ready: {target}")
    log("run: voimed --model-dir " + str(target))
    return 0


if __name__ == "__main__":
    sys.exit(main())
