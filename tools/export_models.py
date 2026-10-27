#!/usr/bin/env python3
"""Convert the official FireRed weights into Voime's ONNX model package.

This script is part of the installation toolchain and never runs at runtime.
It expects the official weight directories laid out as:

    <source-dir>/FireRedASR2-AED/   (HF snapshot: model.pth.tar, cmvn.ark, ...)
    <source-dir>/FireRedPunc/       (HF snapshot: model.pth.tar, ...)
    <source-dir>/vad/stream-vad.onnx + vad/cmvn.ark

The FireRedASR2S Python sources must be importable (`--code-dir` or the
installer adds them to PYTHONPATH).
"""

from __future__ import annotations

import argparse
import json
import shutil
import sys
from pathlib import Path
from typing import Any

import numpy as np

SAMPLE_RATE = 16000


def _fail(message: str) -> None:
    raise SystemExit(f"[export] {message}")


def load_cmvn(path: Path) -> tuple[np.ndarray, np.ndarray]:
    try:
        import kaldiio
    except ImportError as error:  # pragma: no cover - installation-only path
        _fail(f"缺少 kaldiio：{error}")
    stats = kaldiio.load_mat(str(path))
    if stats.shape[0] != 2:
        _fail(f"CMVN 形状非法：{stats.shape}")
    count = float(stats[0, -1])
    if count < 1.0:
        _fail("CMVN count < 1")
    dim = stats.shape[1] - 1
    means = stats[0, :dim] / count
    variances = stats[1, :dim] / count - means * means
    variances = np.maximum(variances, 1e-20)
    istd = 1.0 / np.sqrt(variances)
    return means.astype(np.float32), istd.astype(np.float32)


def _require_torch():
    try:
        import torch
    except ImportError as error:  # pragma: no cover
        _fail(f"缺少 torch：{error}")
    return torch


def _ort_session(path: Path):
    try:
        import onnxruntime as ort
    except ImportError as error:  # pragma: no cover
        _fail(f"缺少 onnxruntime：{error}")
    return ort.InferenceSession(str(path), providers=["CPUExecutionProvider"])


def _feed_shape(shape: list[Any]) -> list[int]:
    result = []
    for dim in shape:
        if isinstance(dim, int) and dim > 0:
            result.append(dim)
        else:
            result.append(1)
    return result


def validate_onnx(path: Path, extra_feeds: dict[str, np.ndarray] | None = None) -> None:
    session = _ort_session(path)
    feeds: dict[str, np.ndarray] = {}
    for value in session.get_inputs():
        if extra_feeds and value.name in extra_feeds:
            feeds[value.name] = extra_feeds[value.name]
            continue
        shape = _feed_shape(value.shape)
        if value.type == "tensor(int64)":
            feeds[value.name] = np.zeros(shape, dtype=np.int64)
        elif value.type == "tensor(float)":
            feeds[value.name] = np.zeros(shape, dtype=np.float32)
        else:
            _fail(f"{path.name} 输入 {value.name} 类型不支持：{value.type}")
    outputs = session.run(None, feeds)
    print(f"[export] validate {path.name}: inputs={[i.name for i in session.get_inputs()]} "
          f"outputs={[o.name for o in session.get_outputs()]} "
          f"shapes={[list(o.shape) for o in outputs]}")


def wrap_vad_with_cmvn(onnx_path: Path, cmvn_path: Path, output: Path) -> None:
    try:
        import onnx
        from onnx import helper, numpy_helper
    except ImportError as error:  # pragma: no cover
        _fail(f"缺少 onnx：{error}")
    means, istd = load_cmvn(cmvn_path)
    model = onnx.load(str(onnx_path))
    graph = model.graph
    feat_name = None
    for value in graph.input:
        dims = [d.dim_value for d in value.type.tensor_type.shape.dim]
        if len(dims) == 3 and dims[-1] == 80:
            feat_name = value.name
            break
    if feat_name is None:
        _fail(f"VAD ONNX 中找不到 [1,T,80] fbank 输入：{onnx_path}")

    graph.initializer.append(numpy_helper.from_array(means, "voime_cmvn_mean"))
    graph.initializer.append(numpy_helper.from_array(istd, "voime_cmvn_istd"))
    sub = helper.make_node(
        "Sub", [feat_name, "voime_cmvn_mean"], ["voime_feat_sub"],
        name="voime_cmvn_sub",
    )
    mul = helper.make_node(
        "Mul", ["voime_feat_sub", "voime_cmvn_istd"], ["voime_feat_norm"],
        name="voime_cmvn_mul",
    )
    for node in graph.node:
        for index, name in enumerate(node.input):
            if name == feat_name:
                node.input[index] = "voime_feat_norm"
    graph.node.insert(0, mul)
    graph.node.insert(0, sub)
    onnx.checker.check_model(model)
    output.parent.mkdir(parents=True, exist_ok=True)
    onnx.save(model, str(output))
    validate_onnx(output)


def export_onnx(torch, module, args, path: Path, **kwargs) -> None:
    """Use the TorchScript ONNX exporter on both older and newer PyTorch."""
    try:
        torch.onnx.export(module, args, str(path), dynamo=False, **kwargs)
    except TypeError as error:
        if "dynamo" not in str(error):
            raise
        torch.onnx.export(module, args, str(path), **kwargs)


def inline_onnx_if_possible(path: Path) -> None:
    """Prefer a self-contained ONNX file so manifest SHA covers all weights."""
    import onnx

    temporary = path.with_name(path.stem + ".inline.onnx")
    if temporary.exists():
        temporary.unlink()
    try:
        model = onnx.load(str(path))
        onnx.save(model, str(temporary))
        if temporary.stat().st_size >= 2 * 1024**3:
            temporary.unlink()
            print(f"[export] {path.name} exceeds 2 GiB; keeping external data")
            return
        temporary.replace(path)
        print(f"[export] inlined {path.name}")
    except Exception as error:
        if temporary.exists():
            temporary.unlink()
        print(f"[export] keep external data for {path.name}: {error}")


def cleanup_export_sidecars(models: list[Path], directory: Path) -> None:
    """Remove external-data shards that no longer belong to the final models."""
    import onnx

    referenced: set[Path] = set()
    for model_path in models:
        model = onnx.load(str(model_path), load_external_data=False)
        for initializer in model.graph.initializer:
            for entry in initializer.external_data:
                if entry.key == "location":
                    referenced.add((model_path.parent / entry.value).resolve())
    for candidate in directory.iterdir():
        if candidate.suffix == ".onnx" or candidate.name == "tokens.txt":
            continue
        if candidate.is_file() and candidate.resolve() not in referenced:
            candidate.unlink()
            print(f"[export] removed stale {candidate.name}")


def export_asr(
    source_dir: Path,
    code_dir: Path,
    out_dir: Path,
    quantize: bool,
) -> None:
    torch = _require_torch()
    import torch.nn.functional as F

    sys.path.insert(0, str(code_dir))
    from fireredasr2s.fireredasr2.asr import FireRedAsr2, FireRedAsr2Config

    config = FireRedAsr2Config(use_gpu=False, use_half=False)
    asr = FireRedAsr2.from_pretrained("aed", str(source_dir), config)
    model = asr.model
    model.eval()

    class EncoderExport(torch.nn.Module):
        def __init__(self, encoder, means, istd):
            super().__init__()
            self.encoder = encoder
            self.register_buffer("cmvn_mean", torch.tensor(means, dtype=torch.float32))
            self.register_buffer("cmvn_istd", torch.tensor(istd, dtype=torch.float32))

        def forward(self, features):
            encoder = self.encoder
            x = (features - self.cmvn_mean) * self.cmvn_istd
            x = F.pad(x, (0, 0, 0, encoder.input_preprocessor.context - 1), "constant", 0.0)
            mask = torch.ones_like(x[..., 0], dtype=torch.uint8).unsqueeze(1)
            embedded, _, src_mask = encoder.input_preprocessor(x, mask)
            output = encoder.dropout(embedded)
            position = encoder.dropout(encoder.positional_encoding(embedded))
            for layer in encoder.layer_stack:
                output = layer(output, position, slf_attn_mask=src_mask, pad_mask=src_mask)
            return output

    class DecoderExport(torch.nn.Module):
        def __init__(self, decoder):
            super().__init__()
            self.decoder = decoder

        def forward(self, tokens, encoder_out):
            decoder = self.decoder
            ones = torch.ones_like(tokens, dtype=torch.int64)
            upper = torch.tril(ones.unsqueeze(2) * ones.unsqueeze(1)).to(torch.float32)
            valid = (tokens != decoder.pad_id).to(torch.float32).unsqueeze(1)
            target_mask = valid * upper
            output = decoder.dropout(
                decoder.tgt_word_emb(tokens) * decoder.scale
                + decoder.positional_encoding(tokens)
            )
            for layer in decoder.layer_stack:
                output = layer.forward(
                    output, encoder_out, target_mask, None, cache=None
                )
            output = decoder.layer_norm_out(output)
            return decoder.tgt_word_prj(output)

    means, istd = load_cmvn(source_dir / "cmvn.ark")
    encoder_path = out_dir / "asr" / "encoder.onnx"
    decoder_path = out_dir / "asr" / "decoder.onnx"
    encoder_path.parent.mkdir(parents=True, exist_ok=True)

    encoder = EncoderExport(model.encoder, means, istd).eval()
    dummy_features = torch.zeros((1, 100, 80), dtype=torch.float32)
    with torch.no_grad():
        export_onnx(
            torch,
            encoder,
            (dummy_features,),
            encoder_path,
            input_names=["features"],
            output_names=["encoder_out"],
            dynamic_axes={"features": {1: "T"}, "encoder_out": {1: "T_out"}},
            opset_version=17,
            do_constant_folding=True,
        )
    validate_onnx(encoder_path)

    decoder = DecoderExport(model.decoder).eval()
    d_model = int(model.encoder.odim)
    sos_id = int(model.decoder.sos_id)
    dummy_tokens = torch.full((1, 2), sos_id, dtype=torch.int64)
    dummy_encoder = torch.zeros((1, 10, d_model), dtype=torch.float32)
    decoder_check = {
        "tokens": np.array([[sos_id, sos_id]], dtype=np.int64),
        "encoder_out": np.zeros((1, 10, d_model), dtype=np.float32),
    }
    with torch.no_grad():
        export_onnx(
            torch,
            decoder,
            (dummy_tokens, dummy_encoder),
            decoder_path,
            input_names=["tokens", "encoder_out"],
            output_names=["logits"],
            dynamic_axes={
                "tokens": {1: "U"},
                "encoder_out": {1: "T", 2: "D"},
                "logits": {1: "U", 2: "V"},
            },
            opset_version=17,
            do_constant_folding=True,
        )
    validate_onnx(decoder_path, extra_feeds=decoder_check)

    if quantize:
        quantize_onnx(encoder_path)
        quantize_onnx(decoder_path)
        inline_onnx_if_possible(encoder_path)
        inline_onnx_if_possible(decoder_path)
        cleanup_export_sidecars([encoder_path, decoder_path], encoder_path.parent)
        validate_onnx(encoder_path)
        validate_onnx(decoder_path, extra_feeds=decoder_check)

    write_tokens(source_dir / "dict.txt", out_dir / "asr" / "tokens.txt")


def write_tokens(dict_path: Path, output: Path) -> None:
    entries: dict[int, str] = {}
    with dict_path.open("r", encoding="utf-8") as handle:
        for raw in handle:
            line = raw.rstrip("\r\n")
            if not line:
                continue
            token, _, index_text = line.rpartition(" ")
            if not token or not index_text.isdigit():
                _fail(f"词表行格式非法：{line!r}")
            entries[int(index_text)] = token
    if not entries:
        _fail(f"词表为空：{dict_path}")
    maximum = max(entries)
    tokens = [""] * (maximum + 1)
    for index, token in entries.items():
        tokens[index] = token
    output.parent.mkdir(parents=True, exist_ok=True)
    output.write_text("\n".join(tokens) + "\n", encoding="utf-8")
    print(f"[export] tokens -> {output} ({len(tokens)} entries)")


def parse_labelled_dict(path: Path) -> dict[int, str]:
    entries: dict[int, str] = {}
    with path.open("r", encoding="utf-8") as handle:
        for raw in handle:
            line = raw.rstrip("\r\n")
            if not line:
                continue
            token, _, index_text = line.rpartition(" ")
            if not token or not index_text.isdigit():
                _fail(f"标签行格式非法：{line!r}")
            entries[int(index_text)] = token
    return entries


def export_punc(
    source_dir: Path,
    code_dir: Path,
    out_dir: Path,
    quantize: bool,
) -> None:
    torch = _require_torch()
    sys.path.insert(0, str(code_dir))
    from fireredasr2s.fireredpunc.punc import FireRedPunc, FireRedPuncConfig

    punc = FireRedPunc.from_pretrained(
        str(source_dir), FireRedPuncConfig(use_gpu=False)
    )
    model = punc.model
    model.eval()

    class PuncExport(torch.nn.Module):
        def __init__(self, punc_model):
            super().__init__()
            self.bert = punc_model.bert
            self.classifier = punc_model.classifier
            self.cls_id = int(punc_model.cls_id or 101)

        def forward(self, input_ids, attention_mask, token_type_ids):
            cls = torch.full_like(input_ids[:, :1], self.cls_id)
            ids = torch.cat([cls, input_ids], dim=1)
            one = torch.ones_like(attention_mask[:, :1])
            mask = torch.cat([one, attention_mask], dim=1)
            zero = torch.zeros_like(token_type_ids[:, :1])
            types = torch.cat([zero, token_type_ids], dim=1)
            hidden = self.bert(ids, attention_mask=mask, token_type_ids=types)[0][:, 1:]
            return self.classifier(hidden)

    punc_path = out_dir / "punc" / "model.onnx"
    punc_path.parent.mkdir(parents=True, exist_ok=True)
    wrapper = PuncExport(model).eval()
    dummy_ids = torch.randint(100, 1000, (1, 8), dtype=torch.int64)
    dummy_mask = torch.ones((1, 8), dtype=torch.int64)
    dummy_types = torch.zeros((1, 8), dtype=torch.int64)
    with torch.no_grad():
        export_onnx(
            torch,
            wrapper,
            (dummy_ids, dummy_mask, dummy_types),
            punc_path,
            input_names=["input_ids", "attention_mask", "token_type_ids"],
            output_names=["logits"],
            dynamic_axes={
                "input_ids": {1: "T"},
                "attention_mask": {1: "T"},
                "token_type_ids": {1: "T"},
                "logits": {1: "T"},
            },
            opset_version=17,
            do_constant_folding=True,
        )
    validate_onnx(punc_path)
    if quantize:
        quantize_onnx(punc_path)
        inline_onnx_if_possible(punc_path)
        cleanup_export_sidecars([punc_path], punc_path.parent)
        validate_onnx(punc_path)

    labels = parse_labelled_dict(source_dir / "out_dict")
    if not labels:
        _fail(f"标点标签为空：{source_dir / 'out_dict'}")
    label_list = [""] * (max(labels) + 1)
    for index, token in labels.items():
        # id 0 is the "no punctuation" class; whitespace labels are also empty.
        label_list[index] = "" if index == 0 or token in {"<space>", " "} else token
    (out_dir / "punc" / "labels.json").write_text(
        json.dumps(label_list, ensure_ascii=False, indent=2) + "\n",
        encoding="utf-8",
    )
    shutil.copy2(
        source_dir / "chinese-bert-wwm-ext_vocab.txt",
        out_dir / "punc" / "vocab.txt",
    )
    print(f"[export] labels -> {out_dir / 'punc' / 'labels.json'} ({len(label_list)} entries)")


def quantize_onnx(path: Path) -> None:
    try:
        from onnxruntime.quantization import QuantType, quantize_dynamic
    except ImportError as error:  # pragma: no cover
        _fail(f"缺少 onnxruntime 量化工具：{error}")
    temporary = path.with_suffix(".int8.onnx")
    quantize_dynamic(str(path), str(temporary), weight_type=QuantType.QInt8)
    temporary.replace(path)
    print(f"[export] quantized {path}")


def export_all(
    *,
    asr_source: Path,
    punc_source: Path,
    vad_onnx: Path,
    vad_cmvn: Path,
    code_dir: Path,
    out_dir: Path,
    quantize: bool,
) -> None:
    out_dir.mkdir(parents=True, exist_ok=True)
    print(f"[export] output package: {out_dir}")
    vad_path = out_dir / "vad" / "stream-vad.onnx"
    wrap_vad_with_cmvn(vad_onnx, vad_cmvn, vad_path)
    if quantize:
        quantize_onnx(vad_path)
        validate_onnx(vad_path)
    export_asr(asr_source, code_dir, out_dir, quantize)
    export_punc(punc_source, code_dir, out_dir, quantize)
    print("[export] DONE")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--asr-source", type=Path, required=True)
    parser.add_argument("--punc-source", type=Path, required=True)
    parser.add_argument("--vad-onnx", type=Path, required=True)
    parser.add_argument("--vad-cmvn", type=Path, required=True)
    parser.add_argument("--code-dir", type=Path, required=True)
    parser.add_argument("--out-dir", type=Path, required=True)
    parser.add_argument("--no-quantize", action="store_true")
    args = parser.parse_args()
    export_all(
        asr_source=args.asr_source,
        punc_source=args.punc_source,
        vad_onnx=args.vad_onnx,
        vad_cmvn=args.vad_cmvn,
        code_dir=args.code_dir,
        out_dir=args.out_dir,
        quantize=not args.no_quantize,
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
