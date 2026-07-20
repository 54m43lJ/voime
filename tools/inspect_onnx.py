"""打印 ONNX 模型的输入/输出签名（名称、dtype、shape）。"""
import sys

import onnx


def show(path):
    m = onnx.load(path, load_external_data=False)
    print(f"== {path}")
    print(f"   ir={m.ir_version} opset={[o.version for o in m.opset_import]}")
    g = m.graph
    print("   inputs:")
    for i in g.input:
        t = i.type.tensor_type
        shape = [d.dim_param or d.dim_value for d in t.shape.dim]
        print(f"     {i.name}: dtype={t.elem_type} shape={shape}")
    print("   outputs:")
    for o in g.output:
        t = o.type.tensor_type
        shape = [d.dim_param or d.dim_value for d in t.shape.dim]
        print(f"     {o.name}: dtype={t.elem_type} shape={shape}")
    print(f"   nodes={len(g.node)} initializer={len(g.initializer)}")


for p in sys.argv[1:]:
    show(p)
