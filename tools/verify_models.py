#!/usr/bin/env python3
"""Verify a converted Voime model package and write its manifest.json."""

from __future__ import annotations

import argparse
import hashlib
import json
import sys
from pathlib import Path
from typing import Any

import numpy as np

DTYPE_MAP = {
    "tensor(float)": "float32",
    "tensor(float16)": "float16",
    "tensor(double)": "float64",
    "tensor(int64)": "int64",
    "tensor(int32)": "int32",
    "tensor(int8)": "int8",
    "tensor(uint8)": "uint8",
}


def fail(message: str) -> None:
    raise SystemExit(f"[verify] {message}")


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        while True:
            block = handle.read(1024 * 1024)
            if not block:
                break
            digest.update(block)
    return digest.hexdigest()


def inspect(path: Path):
    try:
        import onnxruntime as ort
    except ImportError as error:  # pragma: no cover
        fail(f"缺少 onnxruntime：{error}")
    session = ort.InferenceSession(str(path), providers=["CPUExecutionProvider"])
    return session


def specs_from_session(session) -> tuple[list[dict[str, Any]], list[dict[str, Any]]]:
    def convert(values):
        result = []
        for value in values:
            dtype = DTYPE_MAP.get(value.type)
            if dtype is None:
                fail(f"不支持的张量类型：{value.type}")
            shape = []
            for dim in value.shape:
                shape.append(dim if isinstance(dim, int) and dim > 0 else -1)
            result.append({"name": value.name, "dtype": dtype, "shape": shape})
        return result

    return convert(session.get_inputs()), convert(session.get_outputs())


def run(session, feeds: dict[str, np.ndarray]) -> list[np.ndarray]:
    return session.run(None, feeds)


def check_inputs(session, expected: set[str], label: str) -> None:
    actual = {value.name for value in session.get_inputs()}
    missing = expected - actual
    if missing:
        fail(f"{label} 缺少输入：{sorted(missing)} (实际 {sorted(actual)})")


def verify_package(package: Path, write_manifest: bool = True) -> dict[str, Any]:
    package = package.resolve()
    required = [
        "vad/stream-vad.onnx",
        "asr/encoder.onnx",
        "asr/decoder.onnx",
        "asr/tokens.txt",
        "punc/model.onnx",
        "punc/vocab.txt",
        "punc/labels.json",
    ]
    for relative in required:
        path = package / relative
        if not path.is_file() or path.stat().st_size == 0:
            fail(f"模型包缺少文件：{path}")

    vad = inspect(package / "vad/stream-vad.onnx")
    check_inputs(vad, {"feat", "caches_in"}, "VAD")
    vad_inputs, vad_outputs = specs_from_session(vad)
    vad_feeds = {
        "feat": np.zeros((1, 1, 80), dtype=np.float32),
        "caches_in": np.zeros((8, 1, 128, 19), dtype=np.float32),
    }
    vad_result = run(vad, vad_feeds)
    if vad_result[0].shape[1] != 1:
        fail(f"VAD 输出帧数异常：{vad_result[0].shape}")

    encoder = inspect(package / "asr/encoder.onnx")
    check_inputs(encoder, {"features"}, "ASR 编码器")
    encoder_inputs, encoder_outputs = specs_from_session(encoder)
    encoder_result = run(
        encoder, {"features": np.zeros((1, 20, 80), dtype=np.float32)}
    )
    encoder_out = encoder_result[0]
    if encoder_out.ndim != 3:
        fail(f"ASR 编码器输出维数异常：{encoder_out.shape}")

    decoder = inspect(package / "asr/decoder.onnx")
    check_inputs(decoder, {"tokens", "encoder_out"}, "ASR 解码器")
    decoder_inputs, decoder_outputs = specs_from_session(decoder)
    decoder_result = run(
        decoder,
        {
            "tokens": np.array([[1, 2]], dtype=np.int64),
            "encoder_out": encoder_out,
        },
    )
    logits = decoder_result[0]
    if logits.ndim != 3:
        fail(f"ASR 解码器输出维数异常：{logits.shape}")
    tokens = (package / "asr/tokens.txt").read_text(encoding="utf-8").splitlines()
    if not tokens:
        fail("tokens.txt 为空")
    if logits.shape[-1] != len(tokens):
        fail(f"解码器词表 {logits.shape[-1]} != tokens.txt {len(tokens)}")

    punc = inspect(package / "punc/model.onnx")
    check_inputs(punc, {"input_ids", "attention_mask", "token_type_ids"}, "标点模型")
    punc_inputs, punc_outputs = specs_from_session(punc)
    punc_result = run(
        punc,
        {
            "input_ids": np.array([[100, 200, 300, 400]], dtype=np.int64),
            "attention_mask": np.ones((1, 4), dtype=np.int64),
            "token_type_ids": np.zeros((1, 4), dtype=np.int64),
        },
    )
    punc_logits = punc_result[0]
    labels = json.loads((package / "punc/labels.json").read_text(encoding="utf-8"))
    if (
        not isinstance(labels, list)
        or not labels
        or not all(isinstance(label, str) for label in labels)
    ):
        fail("labels.json 必须是字符串数组")
    if punc_logits.shape[-1] != len(labels):
        fail(f"标点类别 {punc_logits.shape[-1]} != labels.json {len(labels)}")

    manifest = {
        "schema": 1,
        "sample_rate": 16000,
        "fbank": {"num_bins": 80, "frame_ms": 25, "shift_ms": 10},
        "max_segment_ms": 10000,
        "vad": {
            "model": "vad/stream-vad.onnx",
            "sha256": sha256(package / "vad/stream-vad.onnx"),
            "inputs": vad_inputs,
            "outputs": vad_outputs,
        },
        "asr": {
            "encoder": {
                "model": "asr/encoder.onnx",
                "sha256": sha256(package / "asr/encoder.onnx"),
                "inputs": encoder_inputs,
                "outputs": encoder_outputs,
            },
            "decoder": {
                "model": "asr/decoder.onnx",
                "sha256": sha256(package / "asr/decoder.onnx"),
                "inputs": decoder_inputs,
                "outputs": decoder_outputs,
            },
            "tokens": "asr/tokens.txt",
        },
        "punc": {
            "model": {
                "model": "punc/model.onnx",
                "sha256": sha256(package / "punc/model.onnx"),
                "inputs": punc_inputs,
                "outputs": punc_outputs,
            },
            "vocab": "punc/vocab.txt",
            "labels": "punc/labels.json",
            "max_tokens": 400,
        },
    }
    if write_manifest:
        path = package / "manifest.json"
        path.write_text(
            json.dumps(manifest, ensure_ascii=False, indent=2) + "\n",
            encoding="utf-8",
        )
        print(f"[verify] manifest -> {path}")
    print("[verify] package OK")
    return manifest


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--model-dir", type=Path, required=True)
    parser.add_argument("--no-write", action="store_true")
    args = parser.parse_args()
    verify_package(args.model_dir, write_manifest=not args.no_write)
    return 0


if __name__ == "__main__":
    sys.exit(main())
