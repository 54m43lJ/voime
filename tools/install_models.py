#!/usr/bin/env python3
"""Install Voime model profiles.

Profiles:
  sensevoice  fixed SenseVoice base package only; Final runs on CPU
  firered     SenseVoice base + FireRedASR2-AED FP16 GPU package

The runtime never calls this script. `scripts/install_models.sh` is the
supported entry point; it only requires Python 3.10+ and network access.
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
    parser.add_argument(
        "--profile",
        choices=["sensevoice", "firered"],
        default="sensevoice",
    )
    parser.add_argument(
        "--model-root",
        "--model-dir",
        dest="model_root",
        type=Path,
        default=default_model_root(),
    )
    parser.add_argument("--cache-dir", type=Path, default=default_cache_dir())
    parser.add_argument("--source-dir", type=Path)
    parser.add_argument(
        "--activate",
        action="store_true",
        help="write models/active; first install also activates by default",
    )
    args = parser.parse_args(argv)

    sys.path.insert(0, str(Path(__file__).resolve().parent))
    from install_sensevoice import PACKAGE_ID, install as install_sensevoice

    root = args.model_root.expanduser().resolve()
    root.mkdir(parents=True, exist_ok=True)

    if args.profile == "sensevoice":
        install_sensevoice(
            model_root=root,
            target=None,
            cache_dir=args.cache_dir,
            source_dir=args.source_dir,
            activate=args.activate,
        )
        log("SenseVoice 安装完成；Final 使用 CPU SenseVoice")
        return 0

    had_active = (root / "active").is_file()
    base_manifest = root / PACKAGE_ID / "manifest.json"
    if not base_manifest.is_file():
        log("FireRed 档需要 SenseVoice 基础包，先安装 base")
        install_sensevoice(
            model_root=root,
            target=None,
            cache_dir=args.cache_dir,
            source_dir=args.source_dir,
            activate=args.activate,
        )
        if not had_active and not args.activate:
            # Let the FireRed package be the first-install selection.
            try:
                (root / "active").unlink()
            except FileNotFoundError:
                pass
    if args.source_dir is None:
        fail(
            "FireRed 档需要 --source-dir 提供导出的 FP16 encoder / decoder "
            "与 FireRedPunc ONNX；见 BUILD §4"
        )
    from install_aed import install as install_aed

    install_aed(
        model_root=root,
        target=None,
        source_dir=args.source_dir,
        activate=args.activate,
    )
    log("FireRed 安装完成；选择 GPU Final 后执行 voime ctl restart-backend")
    return 0


if __name__ == "__main__":
    sys.exit(main())
