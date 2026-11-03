#!/usr/bin/env python3
"""Validate the Voime resource pack layout.

`default/` contains vad + sense-voice. `firered/` contains fire-red-aed +
punc and is optional; when present both models must be complete.
"""

from __future__ import annotations

import hashlib
import json
import sys
from pathlib import Path

SCHEMA = 3
MANIFEST = "manifest.json"


def fail(message: str) -> None:
    raise SystemExit(f"[verify] {message}")


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def resolve(root: Path, relative: str) -> Path:
    path = (root / relative).resolve()
    if root.resolve() not in path.parents and path != root.resolve():
        fail(f"路径越界：{relative}")
    return path


def load(root: Path, kind: str) -> dict:
    path = root / MANIFEST
    if not path.is_file():
        fail(f"缺少 {path}")
    data = json.loads(path.read_text("utf-8"))
    if data.get("schema") != SCHEMA:
        fail(f"{path} schema 必须是 {SCHEMA}")
    if data.get("kind") != kind:
        fail(f"{path} kind 必须是 {kind}")
    return data


def artifact(root: Path, data: dict, label: str) -> None:
    if not isinstance(data, dict) or not data.get("model"):
        fail(f"{label} 缺少 model")
    path = resolve(root, data["model"])
    if not path.is_file() or path.stat().st_size == 0:
        fail(f"{label} 文件缺失或为空：{path}")
    expected = data.get("sha256", "")
    if len(expected) != 64:
        fail(f"{label} SHA-256 格式非法")
    if sha256(path).lower() != expected.lower():
        fail(f"{label} SHA-256 不匹配：{path}")
    if not data.get("inputs") or not data.get("outputs"):
        fail(f"{label} 未声明张量")


def count_tokens(path: Path) -> int:
    count = 0
    for raw in path.read_text("utf-8").splitlines():
        line = raw.strip()
        if not line:
            continue
        _, _, index = line.rpartition(" ")
        if index.isdigit():
            count = max(count, int(index) + 1)
    return count


def verify(root: Path) -> None:
    default = root / "default"
    vad_dir = default / "vad"
    vad = load(vad_dir, "vad")
    artifact(vad_dir, vad.get("model"), "vad.model")
    print("[verify] OK default/vad")

    sense_dir = default / "sense-voice"
    sense = load(sense_dir, "sense-voice")
    artifact(sense_dir, sense.get("model"), "sense-voice.model")
    tokens = resolve(sense_dir, sense.get("tokens", ""))
    if not tokens.is_file():
        fail("sense-voice tokens 缺失")
    if count_tokens(tokens) != sense.get("token_count"):
        fail("sense-voice token_count 不匹配")
    print("[verify] OK default/sense-voice")

    firered = root / "firered"
    if not firered.exists():
        print("[verify] OK 未安装 firered 资源包（CPU 后端可用）")
        return

    fire_dir = firered / "fire-red-aed"
    punc_dir = firered / "punc"
    fire_present = (fire_dir / MANIFEST).is_file()
    punc_present = (punc_dir / MANIFEST).is_file()
    if fire_present != punc_present:
        fail("firered/fire-red-aed 与 firered/punc 必须同时存在或同时缺失")
    if not fire_present:
        print("[verify] OK 未安装 firered 资源包（CPU 后端可用）")
        return

    fire = load(fire_dir, "fire-red-aed")
    artifact(fire_dir, fire.get("encoder"), "fire-red-aed.encoder")
    artifact(fire_dir, fire.get("decoder"), "fire-red-aed.decoder")
    precision = fire.get("precision")
    expected = {"fp16": (7832, 8667), "int8": (7832,)}.get(precision)
    if expected is None:
        fail("fire-red-aed precision 必须是 fp16 或 int8")
    tokens = resolve(fire_dir, fire.get("tokens", ""))
    if count_tokens(tokens) not in expected:
        fail("fire-red-aed token_count 与 precision 不匹配")
    print("[verify] OK fire-red-aed")

    punc = load(punc_dir, "punc")
    artifact(punc_dir, punc.get("model"), "punc.model")
    for key in ("vocab", "labels"):
        if not resolve(punc_dir, punc.get(key, "")).is_file():
            fail(f"punc.{key} 缺失")
    print("[verify] OK punc")


def main() -> int:
    root = Path(sys.argv[1]) if len(sys.argv) > 1 else Path("models")
    verify(root)
    return 0


if __name__ == "__main__":
    sys.exit(main())
