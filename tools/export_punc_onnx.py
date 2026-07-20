"""导出 FireRedPunc 为 ONNX（BERT + 线性分类头，单图）。

输入: input_ids int64 [1,T], attention_mask int64 [1,T]
输出: logits float32 [1,T,5]  (0=空格,1=，2=。3=？4=！)

导出后用 onnxruntime 复算并与 PyTorch FireRedPunc.process 对比校验。
"""
import sys
from pathlib import Path

import numpy as np
import torch

ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(ROOT / "poc" / "python"))

from voime_poc import paths  # noqa: E402
from voime_poc.engines import load_punc  # noqa: E402

OUT_DIR = ROOT / "models" / "FireRedPunc-ONNX"
TEST_TEXTS = [
    "你好世界",
    "今天天气怎么样我们出去走走吧",
    "hello world",
    "你好world这是mixed文本",
    "第一回合开始了吗大家都准备好了吗",
    "what a beautiful day it is today",
]


class PuncExport(torch.nn.Module):
    def __init__(self, model):
        super().__init__()
        self.bert = model.bert
        self.classifier = model.classifier
        self.cls_id = model.cls_id

    def forward(self, input_ids, attention_mask):
        cls = torch.full_like(input_ids[:, :1], self.cls_id)
        ids = torch.cat([cls, input_ids], dim=1)
        m1 = torch.ones_like(attention_mask[:, :1])
        mask = torch.cat([m1, attention_mask], dim=1)
        hidden = self.bert(ids, attention_mask=mask)[0][:, 1:]
        return self.classifier(hidden)


def main() -> int:
    OUT_DIR.mkdir(parents=True, exist_ok=True)
    punc, rep = load_punc()
    print(f"[load] {rep.name} {rep.elapsed_s:.2f}s, cls_id={punc.model.cls_id}, "
          f"odim={punc.model.classifier.out_features}")

    export_model = PuncExport(punc.model).eval()
    onnx_path = OUT_DIR / "punc_bert.onnx"

    dummy_ids = torch.randint(100, 5000, (1, 16), dtype=torch.int64)
    dummy_mask = torch.ones(1, 16, dtype=torch.int64)
    with torch.no_grad():
        torch.onnx.export(
            export_model, (dummy_ids, dummy_mask), str(onnx_path),
            input_names=["input_ids", "attention_mask"],
            output_names=["logits"],
            dynamic_axes={"input_ids": {1: "T"}, "attention_mask": {1: "T"},
                          "logits": {1: "T"}},
            opset_version=17, do_constant_folding=True, dynamo=False)
    print(f"[export] -> {onnx_path} ({onnx_path.stat().st_size / 2**20:.1f} MB)")

    # 同时把词表/out_dict 拷到 ONNX 目录，方便 Rust 侧单目录加载
    import shutil
    shutil.copy(paths.PUNC_MODEL_DIR / "chinese-lert-base" / "tokenizer.json", OUT_DIR)
    shutil.copy(paths.PUNC_MODEL_DIR / "chinese-lert-base" / "vocab.txt", OUT_DIR)
    shutil.copy(paths.PUNC_MODEL_DIR / "out_dict", OUT_DIR)

    # ---- 校验: onnxruntime vs PyTorch ----
    import onnxruntime as ort
    sess = ort.InferenceSession(str(onnx_path), providers=["CPUExecutionProvider"])
    print(f"[check] onnx inputs: {[ (i.name, i.shape) for i in sess.get_inputs() ]}")
    print(f"[check] onnx outputs: {[ (o.name, o.shape) for o in sess.get_outputs() ]}")

    ok = True
    for text in TEST_TEXTS:
        ref = punc.process([text])[0]["punc_text"]
        # 用官方 ModelIO 的 tokenize 路径构造输入
        tokens, _ = punc.model_io.tokenizer.tokenize(text, recover_unk=True)
        ids = [punc.model_io.in_dict.get(t, punc.model_io.in_dict.unk) for t in tokens]
        x = torch.tensor([ids], dtype=torch.int64)
        mask = torch.ones_like(x)
        logits = sess.run(["logits"], {"input_ids": x.numpy(),
                                       "attention_mask": mask.numpy()})[0]
        preds = logits.argmax(-1)[0][:len(ids)].tolist()
        out = punc.model_io.add_punc_to_txt([tokens], [preds])[0]
        from fireredasr2s.fireredpunc.punc import RuleBaedTxtFix
        out = RuleBaedTxtFix.fix(out)
        mark = "OK" if out == ref else "DIFF"
        if out != ref:
            ok = False
        print(f"[check:{mark}] '{text}' -> '{out}' (ref '{ref}')")

    # tokenizer.json 与 in_dict 一致性抽查
    from tokenizers import Tokenizer
    tk = Tokenizer.from_file(str(OUT_DIR / "tokenizer.json"))
    enc = tk.encode("你好world这是mixed文本")
    ids_json = enc.ids
    tokens, _ = punc.model_io.tokenizer.tokenize("你好world这是mixed文本", recover_unk=True)
    ids_ref = [punc.model_io.in_dict.get(t, punc.model_io.in_dict.unk) for t in tokens]
    print(f"[check] tokenizer.json ids == in_dict ids ? {ids_json == ids_ref} "
          f"({ids_json} vs {ids_ref})")
    print("[export] DONE", "ALL OK" if ok else "存在差异")
    return 0 if ok else 1


if __name__ == "__main__":
    raise SystemExit(main())
