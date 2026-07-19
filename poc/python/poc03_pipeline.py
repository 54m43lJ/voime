"""POC-03: VAD -> ASR -> 显示 流水线（实时打印无标点文本）。

验收标准：终端实时打印识别出的无标点文本。
"""
import argparse
import time

from voime_poc.cli import add_source_args, configure_console, run_vad_session
from voime_poc.engines import load_asr, load_stream_vad, transcribe_pcm


def main() -> int:
    configure_console()
    parser = argparse.ArgumentParser(description="POC-03 VAD->ASR->显示 流水线")
    add_source_args(parser)
    parser.add_argument("--beam-size", type=int, default=3)
    args = parser.parse_args()

    print("=" * 64)
    print("POC-03: VAD -> ASR -> 显示 流水线")
    print("=" * 64)

    vad, rep = load_stream_vad(speech_threshold=args.threshold)
    print(f"[OK] {rep.name} 加载耗时 {rep.elapsed_s:.2f}s")
    asr, rep = load_asr(beam_size=args.beam_size)
    print(f"[OK] {rep.name} 加载耗时 {rep.elapsed_s:.2f}s")

    texts = []
    stats = []

    def on_segment_end(seg_pcm, start_s, end_s):
        dur = len(seg_pcm) / 16000
        t0 = time.perf_counter()
        r = transcribe_pcm(asr, seg_pcm, uttid=f"seg{len(texts)}")
        cost = time.perf_counter() - t0
        stats.append((dur, cost))
        texts.append(r["text"])
        print(f"[asr] ({dur:.2f}s 音频, 识别 {cost:.2f}s, rtf={cost / max(dur, 1e-6):.3f})"
              f" >> {r['text']}", flush=True)

    captured, segments = run_vad_session(vad, args, on_segment_end=on_segment_end)

    print("-" * 64)
    print(f"[summary] 句子 {len(segments)} 段，识别文本 {len(texts)} 条:")
    for i, t in enumerate(texts, 1):
        print(f"  #{i}: {t}")
    if stats:
        tot_dur = sum(s[0] for s in stats)
        tot_cost = sum(s[1] for s in stats)
        print(f"[summary] 音频总时长 {tot_dur:.2f}s，ASR 总耗时 {tot_cost:.2f}s，"
              f"平均 rtf={tot_cost / max(tot_dur, 1e-6):.3f}")
    ok = len(texts) > 0 and any(t.strip() for t in texts)
    print(f"[POC-03 {'PASS' if ok else 'FAIL'}] "
          f"{'流水线输出识别文本' if ok else '无有效识别输出'}")
    return 0 if ok else 1


if __name__ == "__main__":
    raise SystemExit(main())
