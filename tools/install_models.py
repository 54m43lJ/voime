#!/usr/bin/env python3
"""Download, convert and atomically install the Voime model package.

The runtime never calls this script. It is invoked by scripts/install_models.sh
(Linux) or scripts/install_models.ps1 (Windows) inside an isolated Python
virtual environment.

When HuggingFace weights are not cached and HF_TOKEN is absent, an interactive
terminal is prompted for a token. Pressing Enter (or running non-interactively)
continues with an unauthenticated download.
"""

from __future__ import annotations

import argparse
import getpass
import os
import shutil
import sys
import tarfile
import tempfile
import urllib.request
from pathlib import Path

ASR_REPO = "FireRedTeam/FireRedASR2-AED"
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
        base = os.environ.get("LOCALAPPDATA") or (
            str(Path.home() / "AppData" / "Local")
        )
        return Path(base) / "voime" / "cache"
    base = os.environ.get("XDG_CACHE_HOME") or str(Path.home() / ".cache")
    return Path(base) / "voime"


def default_model_dir() -> Path:
    if os.name == "nt":
        base = os.environ.get("LOCALAPPDATA") or (
            str(Path.home() / "AppData" / "Local")
        )
        return Path(base) / "voime" / "models"
    base = os.environ.get("XDG_DATA_HOME") or str(
        Path.home() / ".local" / "share"
    )
    return Path(base) / "voime" / "models"


def check_python() -> None:
    if sys.version_info < (3, 10):
        fail(f"需要 Python 3.10+，当前 {sys.version.split()[0]}")


def check_dependencies() -> None:
    missing = []
    for module in (
        "torch",
        "torchaudio",
        "onnx",
        "onnxruntime",
        "numpy",
        "huggingface_hub",
        "transformers",
        "sentencepiece",
        "kaldiio",
        "soundfile",
        "kaldi_native_fbank",
    ):
        try:
            __import__(module)
        except ImportError:
            missing.append(module)
    if missing:
        fail(
            "安装虚拟环境缺少依赖："
            + ", ".join(missing)
            + "；请通过 scripts/install_models.sh 或 install_models.ps1 运行"
        )


def huggingface_cache_ready(cache_dir: Path) -> bool:
    weights = cache_dir / "weights"
    return all(
        (weights / name / ".voime-complete").is_file()
        for name in ("FireRedASR2-AED", "FireRedPunc")
    )


def ensure_hf_token() -> None:
    env_token = (
        os.environ.get("HF_TOKEN", "").strip()
        or os.environ.get("HUGGING_FACE_HUB_TOKEN", "").strip()
    )
    if env_token:
        return

    print(
        "[install] 警告：未检测到 HF_TOKEN，HuggingFace 未认证下载可能被限速。",
        file=sys.stderr,
        flush=True,
    )
    if not sys.stdin.isatty():
        print(
            "[install] 当前不是交互终端，继续未认证下载；"
            "可通过 export HF_TOKEN=... 提供 token。",
            file=sys.stderr,
            flush=True,
        )
        return

    print(
        "[install] 可在此输入 HuggingFace token（https://huggingface.co/settings/tokens）；"
        "直接回车跳过，token 只用于本次安装，不会写入磁盘。",
        file=sys.stderr,
        flush=True,
    )
    try:
        token = getpass.getpass("HF_TOKEN（输入不回显，直接回车跳过）: ").strip()
    except EOFError:
        token = ""
    if token:
        os.environ["HF_TOKEN"] = token
        os.environ["HUGGING_FACE_HUB_TOKEN"] = token
        print("[install] 已使用输入的 token 继续下载。", file=sys.stderr, flush=True)
    else:
        print("[install] 未输入 token，继续未认证下载。", file=sys.stderr, flush=True)


def download(url: str, destination: Path) -> Path:
    destination.parent.mkdir(parents=True, exist_ok=True)
    if destination.is_file() and destination.stat().st_size > 0:
        log(f"cache hit {destination}")
        return destination
    temporary = destination.with_suffix(destination.suffix + ".part")
    log(f"download {url}")
    with urllib.request.urlopen(url, timeout=120) as response, temporary.open("wb") as handle:
        shutil.copyfileobj(response, handle)
    temporary.replace(destination)
    return destination


def snapshot(repo: str, destination: Path, patterns: list[str]) -> Path:
    marker = destination / ".voime-complete"
    if marker.is_file():
        log(f"cache hit {destination}")
        return destination
    from huggingface_hub import snapshot_download

    log(f"download {repo} -> {destination}")
    destination.mkdir(parents=True, exist_ok=True)
    snapshot_download(
        repo_id=repo,
        local_dir=str(destination),
        max_workers=4,
        allow_patterns=patterns,
    )
    marker.write_text("ok\n", encoding="utf-8")
    return destination


def extract_code(cache_dir: Path) -> Path:
    root = cache_dir / "code" / "FireRedASR2S-main"
    if (root / "fireredasr2s").is_dir():
        return root
    archive = cache_dir / "code" / "FireRedASR2S-main.tar.gz"
    download(ASR2S_TARBALL, archive)
    destination = cache_dir / "code"
    destination.mkdir(parents=True, exist_ok=True)
    with tarfile.open(archive, "r:gz") as tar:
        base = destination.resolve()
        for member in tar.getmembers():
            target = (base / member.name).resolve()
            if base not in target.parents and target != base:
                fail(f"压缩包包含越界路径：{member.name}")
        try:
            tar.extractall(destination, filter="data")
        except TypeError:
            # Python 3.10 / 3.11 do not support the filter argument.
            tar.extractall(destination)
    if not (root / "fireredasr2s").is_dir():
        fail(f"FireRedASR2S 源码解压失败：{root}")
    return root


def locate_vad(source_root: Path | None, cache_dir: Path) -> tuple[Path, Path]:
    candidates = []
    if source_root:
        candidates.append(
            source_root
            / "FireRedVAD"
            / "pretrained_models"
            / "onnx_models"
        )
        candidates.append(source_root / "vad")
    for candidate in candidates:
        onnx_path = candidate / "fireredvad_stream_vad_with_cache.onnx"
        cmvn_path = candidate / "cmvn.ark"
        if onnx_path.is_file() and cmvn_path.is_file():
            return onnx_path, cmvn_path
    vad_dir = cache_dir / "vad"
    return (
        download(VAD_ONNX_URL, vad_dir / "fireredvad_stream_vad_with_cache.onnx"),
        download(VAD_CMVN_URL, vad_dir / "cmvn.ark"),
    )


def atomic_install(stage: Path, target: Path) -> None:
    target.parent.mkdir(parents=True, exist_ok=True)
    backup = None
    if target.exists():
        backup = target.with_name(
            f"{target.name}.backup-{os.getpid()}"
        )
        if backup.exists():
            if backup.is_dir():
                shutil.rmtree(backup)
            else:
                backup.unlink()
        os.replace(target, backup)
    try:
        os.replace(stage, target)
    except Exception:
        if backup and backup.exists() and not target.exists():
            os.replace(backup, target)
        raise
    if backup and backup.exists():
        if backup.is_dir():
            shutil.rmtree(backup)
        else:
            backup.unlink()
    log(f"installed -> {target}")


def main() -> int:
    check_python()
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--model-dir", type=Path, default=default_model_dir())
    parser.add_argument("--cache-dir", type=Path, default=default_cache_dir())
    parser.add_argument(
        "--source-dir",
        type=Path,
        help="包含 FireRedASR2-AED/、FireRedPunc/、FireRedVAD/ 的本地目录",
    )
    parser.add_argument("--code-dir", type=Path, help="FireRedASR2S 源码目录")
    parser.add_argument("--no-quantize", action="store_true")
    parser.add_argument("--keep-stage", action="store_true")
    args = parser.parse_args()

    check_dependencies()
    cache_dir = args.cache_dir.resolve()
    cache_dir.mkdir(parents=True, exist_ok=True)
    if args.source_dir is None and not huggingface_cache_ready(cache_dir):
        ensure_hf_token()

    if args.source_dir:
        source_root = args.source_dir.resolve()
        asr_source = source_root / "FireRedASR2-AED"
        punc_source = source_root / "FireRedPunc"
        if not asr_source.is_dir():
            fail(f"缺少 ASR 权重目录：{asr_source}")
        if not punc_source.is_dir():
            fail(f"缺少标点权重目录：{punc_source}")
    else:
        source_root = None
        weights = cache_dir / "weights"
        asr_source = snapshot(
            ASR_REPO,
            weights / "FireRedASR2-AED",
            [
                "model.pth.tar",
                "cmvn.ark",
                "dict.txt",
                "train_bpe1000.model",
                "config.yaml",
            ],
        )
        punc_source = snapshot(
            PUNC_REPO,
            weights / "FireRedPunc",
            [
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

    code_dir = args.code_dir.resolve() if args.code_dir else None
    if code_dir is None:
        if source_root and (source_root / "FireRedASR2S").is_dir():
            code_dir = source_root / "FireRedASR2S"
        else:
            code_dir = extract_code(cache_dir)
    if not (code_dir / "fireredasr2s").is_dir():
        fail(f"找不到 fireredasr2s 源码：{code_dir}")

    vad_onnx, vad_cmvn = locate_vad(source_root, cache_dir)
    target = args.model_dir.expanduser().resolve()
    target.parent.mkdir(parents=True, exist_ok=True)

    stage = Path(
        tempfile.mkdtemp(prefix=".voime-models-", dir=str(target.parent))
    )
    try:
        sys.path.insert(0, str(Path(__file__).resolve().parent))
        sys.path.insert(0, str(code_dir))
        import export_models
        import verify_models

        export_models.export_all(
            asr_source=asr_source,
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
    log("run: voime --model-dir " + str(target))
    return 0


if __name__ == "__main__":
    sys.exit(main())
