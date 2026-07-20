"""对比 Rust 与 Python 的 fbank+cmvn 特征（数值对齐验证）。

用法: compare_feat.py <wav> <rust_feat.bin>
rust_feat.bin 由 `voime_cli feat-dump` 生成（u64 T, u64 80, 后接 T*80 个 f32le）。
"""
import sys
from pathlib import Path

import numpy as np

ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(ROOT / "poc" / "python"))

from voime_poc import paths  # noqa: E402
from voime_poc.audio import read_wav_16k  # noqa: E402

paths.setup_sys_path()
from fireredasr2s.fireredasr2.data.asr_feat import ASRFeatExtractor  # noqa: E402


def main():
    wav, bin_path = Path(sys.argv[1]), Path(sys.argv[2])
    pcm = read_wav_16k(wav)

    # Python 参考（官方 ASRFeatExtractor: knf + cmvn）
    ext = ASRFeatExtractor(str(paths.ASR_MODEL_DIR / "cmvn.ark"))
    feats, lengths, durs, _, _ = ext([(16000, pcm)], ["t"])
    ref = feats[0].numpy()  # (T, 80)

    # Rust 特征
    raw = np.fromfile(bin_path, dtype=np.uint8)
    t, d = np.frombuffer(raw[:16], dtype=np.uint64)
    rust = np.frombuffer(raw[16:], dtype=np.float32).reshape(int(t), int(d))

    print(f"ref shape={ref.shape}, rust shape={rust.shape}")
    if ref.shape != rust.shape:
        n = min(ref.shape[0], rust.shape[0])
        ref, rust = ref[:n], rust[:n]
        print(f"[warn] 帧数不一致，截断到 {n}")
    diff = np.abs(ref - rust)
    print(f"max abs diff = {diff.max():.6f}")
    print(f"mean abs diff = {diff.mean():.8f}")
    print(f"ref range [{ref.min():.3f},{ref.max():.3f}] rust range [{rust.min():.3f},{rust.max():.3f}]")
    ok = diff.max() < 1e-3
    print(f"[{'PASS' if ok else 'FAIL'}] 阈值 1e-3")
    return 0 if ok else 1


if __name__ == "__main__":
    raise SystemExit(main())
