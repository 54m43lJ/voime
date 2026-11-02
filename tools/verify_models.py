#!/usr/bin/env python3
"""Validate a schema 3 Voime model package."""

from __future__ import annotations

import hashlib
import json
import sys
from pathlib import Path

MANIFEST_FILE = "manifest.json"
SCHEMA = 3


def fail(message: str) -> None:
    raise SystemExit(f"[verify] {message}")


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def resolve_inside(root: Path, relative: str) -> Path:
    candidate = (root / relative).resolve()
    if root.resolve() not in candidate.parents and candidate != root.resolve():
        fail(f"manifest 路径越界：{relative}")
    return candidate


def count_tokens(path: Path) -> int:
    maximum = -1
    seen: set[int] = set()
    for line_number, raw in enumerate(path.read_text(encoding="utf-8").splitlines(), 1):
        line = raw.strip()
        if not line:
            continue
        _, _, index_text = line.rpartition(" ")
        if not index_text.isdigit():
            fail(f"tokens.txt:{line_number} 格式非法：{raw!r}")
        index = int(index_text)
        if index in seen:
            fail(f"tokens.txt:{line_number} id 重复：{index}")
        seen.add(index)
        maximum = max(maximum, index)
    if maximum < 0:
        fail("tokens.txt 为空")
    return maximum + 1


def verify(root: Path) -> dict:
    root = root.resolve()
    manifest_path = root / MANIFEST_FILE
    if not manifest_path.is_file():
        fail(f"缺少 {manifest_path}")
    manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
    if manifest.get("schema") != SCHEMA:
        fail(f"schema 必须是 {SCHEMA}，实际 {manifest.get('schema')}")
    if manifest.get("sample_rate") != 16000:
        fail("sample_rate 必须是 16000")
    for key in ("vad",):
        artifact = manifest.get(key)
        if not isinstance(artifact, dict):
            fail(f"缺少 {key} 段")
        verify_artifact(root, artifact)
    asr = manifest.get("asr")
    if not isinstance(asr, dict):
        fail("缺少 asr 段")
    kind = asr.get("kind")
    if kind == "sense-voice":
        verify_artifact(root, asr.get("model", {}))
    elif kind == "fire-red-aed":
        verify_artifact(root, asr.get("encoder", {}))
        verify_artifact(root, asr.get("decoder", {}))
        if asr.get("precision") != "fp16":
            fail("fire-red-aed 必须使用 precision=fp16")
        if manifest.get("backend", {}).get("kind") != "worker":
            fail("fire-red-aed 必须使用 backend.kind=worker")
        if "sense-voice-2024-int8" not in manifest.get("requires", []):
            fail("fire-red-aed 必须 requires sense-voice-2024-int8")
    else:
        fail(f"不支持的 asr.kind：{kind}")
    tokens = resolve_inside(root, asr.get("tokens", ""))
    if not tokens.is_file():
        fail(f"缺少 tokens：{tokens}")
    found = count_tokens(tokens)
    if found != asr.get("token_count"):
        fail(f"tokens 数量 {found} 与清单 {asr.get('token_count')} 不一致")
    punc = manifest.get("punc")
    if asr.get("options", {}).get("punc") == "external":
        if not isinstance(punc, dict):
            fail("external 标点缺少 punc 段")
        verify_artifact(root, punc.get("model", {}))
        for key in ("vocab", "labels"):
            if not resolve_inside(root, punc.get(key, "")).is_file():
                fail(f"缺少 punc.{key}")
    print(f"[verify] OK {manifest.get('display_name')} ({kind})")
    return manifest


def verify_artifact(root: Path, artifact: dict) -> None:
    if not isinstance(artifact, dict) or not artifact.get("model"):
        fail("artifact 缺少 model")
    path = resolve_inside(root, artifact["model"])
    if not path.is_file() or path.stat().st_size == 0:
        fail(f"模型文件缺失或为空：{path}")
    expected = artifact.get("sha256", "")
    if len(expected) != 64:
        fail(f"SHA-256 格式非法：{path}")
    actual = sha256(path)
    if actual.lower() != expected.lower():
        fail(f"SHA-256 不匹配：{path}")
    if not artifact.get("inputs") or not artifact.get("outputs"):
        fail(f"{path} 未声明输入 / 输出张量")


def main() -> int:
    root = Path(sys.argv[1]) if len(sys.argv) > 1 else Path("models")
    verify(root)
    return 0


if __name__ == "__main__":
    sys.exit(main())
