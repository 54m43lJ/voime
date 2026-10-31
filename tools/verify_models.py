#!/usr/bin/env python3
"""Validate a staged Voime model package and write manifest.json (schema 2)."""

from __future__ import annotations

import hashlib
import json
import sys
from pathlib import Path
from typing import Any

import numpy as np

MANIFEST_FILE = "manifest.json"


def fail(message: str) -> None:
    raise SystemExit(f"[verify] {message}")


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def dtype_name(value_type: str) -> str:
    return {
        "tensor(float)": "float32",
        "tensor(double)": "float64",
        "tensor(int64)": "int64",
        "tensor(int32)": "int32",
        "tensor(int8)": "int8",
        "tensor(uint8)": "uint8",
    }.get(value_type, value_type)


def describe_tensor(value: Any) -> dict[str, Any]:
    shape = [dim if isinstance(dim, int) and dim > 0 else -1 for dim in value.shape]
    return {"name": value.name, "dtype": dtype_name(value.type), "shape": shape}


def session(path: Path):
    import onnxruntime as ort

    return ort.InferenceSession(str(path), providers=["CPUExecutionProvider"])


def artifact(path: Path) -> dict[str, Any]:
    runtime = session(path)
    return {
        "model": str(path),
        "sha256": sha256(path),
        "inputs": [describe_tensor(value) for value in runtime.get_inputs()],
        "outputs": [describe_tensor(value) for value in runtime.get_outputs()],
    }


def feeds_for(runtime, kind: str, frames: int = 100) -> dict[str, np.ndarray]:
    feeds: dict[str, np.ndarray] = {}
    for value in runtime.get_inputs():
        shape = [dim if isinstance(dim, int) and dim > 0 else 1 for dim in value.shape]
        if kind == "punc" and len(shape) == 2:
            shape = [shape[0], 8]
        name = value.name
        if value.type == "tensor(int64)":
            data = np.zeros(shape, dtype=np.int64)
            if kind == "asr" and len(shape) == 1:
                data[0] = frames
            elif kind == "punc":
                if "token_type" in name:
                    # token_type_ids are 0/1 sentence ids, never token ids.
                    data.fill(0)
                elif "mask" in name:
                    data.fill(1)
                elif name == "input_ids" or name.endswith("_input_ids"):
                    rng = np.random.default_rng(0)
                    data = rng.integers(100, 1000, size=shape, dtype=np.int64)
            feeds[name] = data
        elif value.type == "tensor(float)":
            if kind == "asr" and len(shape) == 3 and shape[-1] == 80:
                feeds[name] = np.zeros((1, frames, 80), dtype=np.float32)
            elif kind == "vad" and len(shape) == 3 and shape[-1] == 80:
                feeds[name] = np.zeros((1, frames, 80), dtype=np.float32)
            else:
                feeds[name] = np.zeros(shape, dtype=np.float32)
        else:
            fail(f"{kind} 输入 {name} 类型不支持：{value.type}")
    return feeds


def verify_runtime(path: Path, kind: str) -> None:
    runtime = session(path)
    feeds = feeds_for(runtime, kind)
    outputs = runtime.run(None, feeds)
    print(
        f"[verify] {kind}: {path.name} "
        f"outputs={[list(output.shape) for output in outputs]}"
    )


def count_tokens(path: Path) -> int:
    maximum = -1
    seen: set[int] = set()
    for line_number, raw in enumerate(path.read_text(encoding="utf-8").splitlines(), 1):
        line = raw.strip()
        if not line:
            continue
        token, _, index_text = line.rpartition(" ")
        if not token or not index_text.isdigit():
            fail(f"tokens.txt:{line_number} 格式非法：{raw!r}")
        index = int(index_text)
        if index in seen:
            fail(f"tokens.txt:{line_number} id 重复：{index}")
        seen.add(index)
        maximum = max(maximum, index)
    if maximum < 0:
        fail("tokens.txt 为空")
    return maximum + 1


def find_artifact(root: Path, kind: str, name: str) -> Path:
    direct = root / kind / name
    if direct.is_file():
        return direct
    matches = list(root.rglob(name))
    if not matches:
        fail(f"模型包缺少 {kind}/{name}")
    return matches[0]


def verify_package(root: Path, write_manifest: bool = True) -> dict[str, Any]:
    root = root.resolve()
    vad_path = find_artifact(root, "vad", "stream-vad.onnx")
    asr_path = find_artifact(root, "asr", "model.int8.onnx")
    tokens = find_artifact(root, "asr", "tokens.txt")
    punc_path = find_artifact(root, "punc", "model.onnx")
    vocab = find_artifact(root, "punc", "vocab.txt")
    labels_path = find_artifact(root, "punc", "labels.json")

    labels = json.loads(labels_path.read_text(encoding="utf-8"))
    if not isinstance(labels, list) or not labels:
        fail(f"标点标签格式非法：{labels_path}")
    token_count = count_tokens(tokens)

    vad = artifact(vad_path)
    asr_model = artifact(asr_path)
    punc_model = artifact(punc_path)

    log_probs_shapes = [
        output["shape"]
        for output in asr_model["outputs"]
        if output["name"] == "log_probs"
    ]
    if not log_probs_shapes:
        fail("ASR 模型缺少 log_probs 输出")
    if log_probs_shapes[0][-1] > 0 and log_probs_shapes[0][-1] != token_count:
        fail(
            f"tokens.txt 有 {token_count} 项，但 ASR 输出维度为 "
            f"{log_probs_shapes[0][-1]}"
        )

    verify_runtime(vad_path, "vad")
    verify_runtime(asr_path, "asr")
    verify_runtime(punc_path, "punc")

    def relative(path: Path) -> str:
        return path.relative_to(root).as_posix()

    manifest = {
        "schema": 2,
        "sample_rate": 16000,
        "fbank": {"num_bins": 80, "frame_ms": 25, "shift_ms": 10},
        "max_segment_ms": 30000,
        "vad": {
            "model": relative(vad_path),
            "sha256": vad["sha256"],
            "inputs": vad["inputs"],
            "outputs": vad["outputs"],
        },
        "asr": {
            "kind": "fire-red-ctc",
            "model": {
                "model": relative(asr_path),
                "sha256": asr_model["sha256"],
                "inputs": asr_model["inputs"],
                "outputs": asr_model["outputs"],
            },
            "tokens": relative(tokens),
            "token_count": token_count,
        },
        "punc": {
            "model": {
                "model": relative(punc_path),
                "sha256": punc_model["sha256"],
                "inputs": punc_model["inputs"],
                "outputs": punc_model["outputs"],
            },
            "vocab": relative(vocab),
            "labels": relative(labels_path),
            "max_tokens": 400,
        },
    }

    if write_manifest:
        (root / MANIFEST_FILE).write_text(
            json.dumps(manifest, ensure_ascii=False, indent=2) + "\n",
            encoding="utf-8",
        )
        print(f"[verify] wrote {root / MANIFEST_FILE}")
    return manifest


def main() -> int:
    root = Path(sys.argv[1]) if len(sys.argv) > 1 else Path("models")
    verify_package(root, write_manifest=True)
    return 0


if __name__ == "__main__":
    sys.exit(main())
