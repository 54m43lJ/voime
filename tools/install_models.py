#!/usr/bin/env python3
"""Install Voime model directories.

Default: FireRedVAD + SenseVoice (`models/vad`, `models/sense-voice`).
With --gpu: also install FireRed AED + FireRedPunc
(`models/fire-red-aed`, `models/punc`).

The daemon always loads VAD and SenseVoice; the GPU pair is used as Final only
when both directories exist.
"""

from __future__ import annotations

import argparse
import sys
from pathlib import Path


def log(message: str) -> None:
    print(f"[install] {message}", flush=True)


def fail(message: str) -> None:
    raise SystemExit(f"[install] {message}")


def default_model_root() -> Path:
    import os

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
    import os

    if os.name == "nt":
        base = os.environ.get("LOCALAPPDATA") or str(
            Path.home() / "AppData" / "Local"
        )
    else:
        base = os.environ.get("XDG_CACHE_HOME") or str(Path.home() / ".cache")
    return Path(base) / "voime"


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--gpu", action="store_true", help="also install FireRed AED + Punc")
    parser.add_argument(
        "--model-root",
        "--model-dir",
        dest="model_root",
        type=Path,
        default=default_model_root(),
    )
    parser.add_argument("--cache-dir", type=Path, default=default_cache_dir())
    parser.add_argument("--source-dir", type=Path, help="local exported FireRed FP16 directory")
    parser.add_argument(
        "--punc-source-dir",
        "--punc-dir",
        dest="punc_source_dir",
        type=Path,
        help="directory containing punc/model.onnx, vocab.txt, labels.json",
    )
    parser.add_argument(
        "--aed-version",
        choices=["v1", "v2", "auto"],
        default="v1",
        help="v1 = official FP16 package (temporary default), v2 = local fp16 export",
    )
    args = parser.parse_args(argv)

    sys.path.insert(0, str(Path(__file__).resolve().parent))
    from install_sensevoice import install as install_sensevoice
    from install_vad import install as install_vad

    root = args.model_root.expanduser().resolve()
    root.mkdir(parents=True, exist_ok=True)

    install_vad(model_root=root, cache_dir=args.cache_dir, source_dir=args.source_dir)
    install_sensevoice(model_root=root, cache_dir=args.cache_dir)
    log("默认模型安装完成：VAD + SenseVoice（CPU Final）")

    if not args.gpu:
        return 0

    punc_source = args.punc_source_dir or args.source_dir
    have_punc = (root / "punc" / "manifest.json").is_file()
    if punc_source is None and not have_punc:
        fail(
            "--gpu 需要 --punc-source-dir 指向导出的 FireRedPunc 目录"
            "（model.onnx / vocab.txt / labels.json）；"
            "如果 --source-dir 同时包含 punc/ 也可以复用它"
        )
    from install_firered import install as install_firered
    from install_punc import install as install_punc

    install_firered(
        model_root=root,
        cache_dir=args.cache_dir,
        source_dir=args.source_dir,
        variant=args.aed_version,
    )
    if punc_source is not None:
        install_punc(
            model_root=root,
            source_dir=punc_source,
        )
    else:
        log("复用已安装的 models/punc")
    log("GPU 模型安装完成：fire-red-aed + punc")
    log("重启 voimed 后生效；启动时会自动选择 GPU Final")
    return 0


if __name__ == "__main__":
    sys.exit(main())
