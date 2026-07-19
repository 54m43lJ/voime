"""POC-04: PTT 松开后调用 FireRedPunc，输出带标点的完整文本。

验收标准：输出带标点的完整文本。
- mic 模式：回车开始（PTT 按下），再回车结束（PTT 松开）。
- wav 模式：整个文件视为一次 PTT 会话。
"""
import argparse
import re
import threading
import time

from voime_poc.cli import add_source_args, configure_console, run_vad_session
from voime_poc.engines import load_asr, load_punc, load_stream_vad, punc_text, transcribe_pcm


def smart_join(texts):
    """拼接句段文本：相邻英文/数字 token 之间补空格，中文直连。"""
    out = ""
    for t in texts:
        t = t.strip()
        if not t:
            continue
        if out and re.match(r"[a-zA-Z0-9]", t[0]) and re.search(r"[a-zA-Z0-9]$", out):
            out += " "
        out += t
    return out


def main() -> int:
    configure_console()
    parser = argparse.ArgumentParser(description="POC-04 PTT 松开后标点补全")
    add_source_args(parser)
    parser.add_argument("--beam-size", type=int, default=3)
    args = parser.parse_args()

    print("=" * 64)
    print("POC-04: PTT 会话 -> VAD -> ASR -> (松开) Punc 完整输出")
    print("=" * 64)

    vad, rep = load_stream_vad(speech_threshold=args.threshold)
    print(f"[OK] {rep.name} 加载耗时 {rep.elapsed_s:.2f}s")
    asr, rep = load_asr(beam_size=args.beam_size)
    print(f"[OK] {rep.name} 加载耗时 {rep.elapsed_s:.2f}s")
    punc, rep = load_punc()
    print(f"[OK] {rep.name} 加载耗时 {rep.elapsed_s:.2f}s")

    texts = []

    def on_segment_end(seg_pcm, start_s, end_s):
        r = transcribe_pcm(asr, seg_pcm, uttid=f"seg{len(texts)}")
        texts.append(r["text"])
        print(f"[asr] 实时句段 >> {r['text']}", flush=True)

    # PTT 模拟
    stop_evt = threading.Event()
    if args.source == "mic":
        print("[ptt] 按回车开始说话（PTT 按下）...", flush=True)
        input()
        print("[ptt] 录音中，再按回车结束（PTT 松开）", flush=True)

        def wait_enter():
            input()
            stop_evt.set()

        threading.Thread(target=wait_enter, daemon=True).start()
    else:
        print("[ptt] wav 模式：整段文件 = 一次 PTT 会话", flush=True)

    t_ptt_start = time.perf_counter()
    captured, segments = run_vad_session(
        vad, args, on_segment_end=on_segment_end,
        stop_flag=stop_evt.is_set if args.source == "mic" else None)
    t_ptt_release = time.perf_counter()
    print(f"[ptt] 会话时长 {t_ptt_release - t_ptt_start:.2f}s，句段 {len(segments)} 段")

    # PTT 松开：拼接全文 -> Punc
    full_text = smart_join(texts)
    print("-" * 64)
    print(f"[punc] 输入（无标点）: {full_text}")
    t0 = time.perf_counter()
    punctuated = punc_text(punc, full_text) if full_text else ""
    punc_cost = time.perf_counter() - t0
    print(f"[punc] 耗时 {punc_cost:.3f}s")
    print("=" * 64)
    print(f"[最终输出] {punctuated}")
    print("=" * 64)

    ok = bool(punctuated.strip()) and any(p in punctuated for p in "。，？！,.?!")
    print(f"[POC-04 {'PASS' if ok else 'FAIL'}] "
          f"{'输出带标点完整文本' if ok else '输出为空或无标点'}")
    return 0 if ok else 1


if __name__ == "__main__":
    raise SystemExit(main())
