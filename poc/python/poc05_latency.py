"""POC-05: 各模块延迟测量（VAD / ASR 单句 / Punc），对照 <500ms 目标。

测试集：FireRedASR2S 自带 hello_zh/hello_en，及拼接生成的多句长音频。
测量：
  - VAD 流式逐帧耗时（对比 25ms/帧 实时预算）
  - 各句段 ASR 耗时（median of N reps），含 beam_size 对比
  - 全文 Punc 耗时
  - 端到端 PTT 延迟估算 = ASR(最后一句) + Punc(全文)
输出：控制台表格 + out/poc05_latency.json
"""
import argparse
import json
import statistics
import time

import numpy as np
import psutil

from voime_poc import paths
from voime_poc.audio import read_wav_16k, write_wav_16k
from voime_poc.cli import configure_console
from voime_poc.engines import load_asr, load_punc, load_stream_vad, punc_text, transcribe_pcm
from voime_poc.vad_session import VadSession


def build_test_set():
    """构造测试音频：原生 2s 短句 + 拼接多句中长音频。"""
    out = paths.ensure_out_dir() / "latency"
    out.mkdir(parents=True, exist_ok=True)
    zh = read_wav_16k(paths.ASSETS_DIR / "hello_zh.wav")
    en = read_wav_16k(paths.ASSETS_DIR / "hello_en.wav")
    gap = np.zeros(int(0.7 * 16000), dtype=np.int16)

    def repeat(pcm, n):
        parts = []
        for i in range(n):
            parts.append(pcm)
            if i != n - 1:
                parts.append(gap)
        return np.concatenate(parts)

    cases = {}
    for name, pcm, n in [("zh_1x2s", zh, 1), ("zh_2x2s", zh, 2), ("zh_4x2s", zh, 4),
                         ("en_1x2s", en, 1), ("en_2x2s", en, 2), ("en_4x2s", en, 4)]:
        data = repeat(pcm, n)
        p = out / f"{name}.wav"
        write_wav_16k(p, data)
        cases[name] = data
    return cases


def measure_vad_streaming(vad, pcm_i16):
    """流式 VAD 全量过一遍：返回 (总耗时, 帧数, 句段列表)。"""
    session = VadSession(vad)
    t0 = time.perf_counter()
    segments = []
    for ev in session.push(pcm_i16):
        if ev.kind == "end":
            segments.append((ev.start_sample, ev.end_sample))
    ev = session.flush()
    if ev is not None:
        segments.append((ev.start_sample, ev.end_sample))
    cost = time.perf_counter() - t0
    return cost, session.frames_fed, segments


def med(fn, reps):
    ts = []
    r = None
    for _ in range(reps):
        t0 = time.perf_counter()
        r = fn()
        ts.append(time.perf_counter() - t0)
    return statistics.median(ts), min(ts), r


def main() -> int:
    configure_console()
    parser = argparse.ArgumentParser(description="POC-05 延迟测量")
    parser.add_argument("--beam-sizes", default="3,1", help="逗号分隔，如 '3,1'")
    parser.add_argument("--reps", type=int, default=3, help="每次测量重复数取中位数")
    parser.add_argument("--out", default=str(paths.OUT_DIR / "poc05_latency.json"))
    args = parser.parse_args()

    proc = psutil.Process()
    rss0 = proc.memory_info().rss / 2**20

    print("=" * 72)
    print("POC-05: VAD / ASR / Punc 延迟测量（CPU, fp32）")
    print("=" * 72)

    vad, rep_vad = load_stream_vad()
    punc, rep_punc = load_punc()
    rss_models = proc.memory_info().rss / 2**20
    print(f"[mem] 加载 VAD+Punc 后 RSS: {rss_models:.0f} MB (基线 {rss0:.0f} MB)")

    cases = build_test_set()

    # VAD 流式耗时（与 beam 无关，测一次）
    print("\n--- VAD 流式逐帧耗时 ---")
    vad_stats = {}
    for name, pcm in cases.items():
        cost, frames, segments = measure_vad_streaming(vad, pcm)
        vad_stats[name] = {"cost_s": cost, "frames": frames, "segments": segments,
                           "dur_s": len(pcm) / 16000}
        print(f"  {name}: {len(pcm) / 16000:5.2f}s 音频, {frames} 帧, "
              f"VAD 总耗时 {cost * 1000:6.1f}ms, "
              f"均帧 {cost / max(frames, 1) * 1000:.3f}ms/帧 (预算 25ms), "
              f"句段 {len(segments)}")

    results = {"load": {"vad_s": rep_vad.elapsed_s, "punc_s": rep_punc.elapsed_s},
               "vad": {k: {"cost_s": v["cost_s"], "frames": v["frames"],
                           "n_segments": len(v["segments"])} for k, v in vad_stats.items()},
               "asr": {}, "punc": {}, "e2e": {}}

    for beam_size in [int(x) for x in args.beam_sizes.split(",")]:
        asr, rep_asr = load_asr(beam_size=beam_size)
        rss_asr = proc.memory_info().rss / 2**20
        print(f"\n[mem] 加载 ASR(beam={beam_size}) 后 RSS: {rss_asr:.0f} MB "
              f"(+{rss_asr - rss_models:.0f} MB)")
        results["load"][f"asr_beam{beam_size}_s"] = rep_asr.elapsed_s
        results.setdefault("mem_mb", {})[f"rss_after_asr_beam{beam_size}"] = round(rss_asr, 1)

        # 预热一次（oneDNN/算子初始化）
        first_pcm = next(iter(cases.values()))
        segs = vad_stats[next(iter(cases))]["segments"]
        s, e = segs[0]
        transcribe_pcm(asr, first_pcm[s:e])

        print(f"\n--- ASR 单句耗时 (beam_size={beam_size}, reps={args.reps}) ---")
        for name, pcm in cases.items():
            segs = vad_stats[name]["segments"]
            seg_costs = []
            texts = []
            for i, (s, e) in enumerate(segs):
                seg = pcm[s:e]
                dur = len(seg) / 16000
                cost, best, r = med(lambda: transcribe_pcm(asr, seg, f"{name}_{i}"),
                                    args.reps)
                seg_costs.append({"dur_s": round(dur, 3), "cost_s": round(cost, 3),
                                  "rtf": round(cost / max(dur, 1e-6), 3)})
                texts.append(r["text"])
            # 全文 Punc
            full_text = "".join(texts)
            punc_cost, _, punctuated = med(lambda: punc_text(punc, full_text), args.reps)
            # 端到端 PTT 延迟 = 最后一句 ASR + 全文 Punc
            last_asr = seg_costs[-1]["cost_s"] if seg_costs else 0
            e2e = last_asr + punc_cost
            key = f"{name}@beam{beam_size}"
            results["asr"][key] = seg_costs
            results["punc"][key] = {"chars": len(full_text), "cost_s": round(punc_cost, 3)}
            results["e2e"][key] = {"last_asr_s": round(last_asr, 3),
                                   "punc_s": round(punc_cost, 3),
                                   "total_s": round(e2e, 3),
                                   "target_0.5s": e2e < 0.5}
            tot_asr = sum(c["cost_s"] for c in seg_costs)
            print(f"  {name} ({vad_stats[name]['dur_s']:.1f}s, {len(segs)} 句): "
                  f"ASR 总 {tot_asr:.2f}s | 末句 {last_asr:.2f}s | "
                  f"Punc {punc_cost * 1000:.0f}ms | PTT端到端 {e2e * 1000:.0f}ms "
                  f"{'<500ms OK' if e2e < 0.5 else '>=500ms 超标'}")
            print(f"    文本: {punctuated}")
        del asr  # 释放再测下一个 beam

    with open(args.out, "w", encoding="utf-8") as f:
        json.dump(results, f, ensure_ascii=False, indent=2)
    print(f"\n[save] 结果已写入 {args.out}")

    any_ok = any(v["target_0.5s"] for v in results["e2e"].values())
    print(f"[POC-05 {'PASS' if any_ok else 'FAIL'}] "
          f"{'存在满足 <500ms 的配置' if any_ok else '所有配置均 >=500ms'}")
    return 0 if any_ok else 1


if __name__ == "__main__":
    raise SystemExit(main())
