"""POC-02/03/04 共享命令行与采集会话逻辑。"""
import argparse
import sys
import time

import numpy as np

from . import paths
from .audio import MicSource, WavSource, float_to_i16, read_wav_16k, write_wav_16k
from .vad_session import VadSession


def add_source_args(parser: argparse.ArgumentParser) -> None:
    parser.add_argument("--source", choices=["mic", "wav"], default="wav",
                        help="音频来源：mic=默认麦克风实时采集；wav=文件模拟实时流")
    parser.add_argument("--wav", default=str(paths.ASSETS_DIR / "hello_zh.wav"),
                        help="--source=wav 时的输入文件（自动重采样到 16kHz）")
    parser.add_argument("--realtime", action=argparse.BooleanOptionalAction, default=True,
                        help="wav 模式是否按真实时间节奏喂数据（默认开）")
    parser.add_argument("--max-sec", type=float, default=0,
                        help="采集时长上限（秒），0=不限/wav 播完即止")
    parser.add_argument("--save-capture", default="",
                        help="把本次采集到的 16kHz PCM 存为 wav 文件")
    parser.add_argument("--threshold", type=float, default=0.4,
                        help="VAD 语音判定阈值 speech_threshold")


def open_source(args):
    """返回 (source, description)。source 统一 read()->float32[-1,1] 接口。"""
    if args.source == "mic":
        src = MicSource().start()
        desc = f"麦克风: {src.device_name} (device {src.device_sr}Hz -> 16kHz)"
    else:
        pcm = read_wav_16k(args.wav)
        src = WavSource(pcm, realtime=args.realtime).start()
        desc = f"文件: {args.wav} ({len(pcm) / 16000:.2f}s, realtime={args.realtime})"
    return src, desc


def run_vad_session(vad, args, on_segment_end=None, stop_flag=None):
    """通用采集+VAD 循环。

    on_segment_end(pcm_seg, start_s, end_s): 句段闭合回调（POC-03 用它触发 ASR）。
    stop_flag(): 返回 True 时提前结束（POC-04 的 PTT 松开）。
    返回 (captured_i16, segments[(s,e)], sentence_events)。
    """
    session = VadSession(vad)
    captured = []
    segments = []
    t_start = time.perf_counter()
    src, desc = open_source(args)
    print(f"[source] {desc}", flush=True)
    idle_polls = 0
    try:
        while True:
            if stop_flag is not None and stop_flag():
                print("[ptt] 停止信号，结束采集", flush=True)
                break
            if args.max_sec and (time.perf_counter() - t_start) > args.max_sec:
                print(f"[limit] 达到 {args.max_sec}s 上限", flush=True)
                break
            blk = src.read()
            if len(blk) == 0:
                if args.source == "wav" and src.exhausted:
                    break
                idle_polls += 1
                if args.source == "mic" and idle_polls > 50:
                    print("[warn] 麦克风连续 5s 无数据，检查设备", flush=True)
                    idle_polls = 0
                continue
            pcm = float_to_i16(blk)
            captured.append(pcm)
            base = session.fed_samples
            for ev in session.push(pcm):
                if ev.kind == "start":
                    print(f"[vad] Speech Start @ {ev.start_sample / 16000:.2f}s", flush=True)
                else:
                    segments.append((ev.start_sample, ev.end_sample))
                    dur = (ev.end_sample - ev.start_sample) / 16000
                    print(f"[vad] Sentence Detected: {ev.start_sample / 16000:.2f}s"
                          f" ~ {ev.end_sample / 16000:.2f}s (时长 {dur:.2f}s)", flush=True)
                    if on_segment_end is not None:
                        full = np.concatenate(captured)
                        seg_pcm = full[ev.start_sample:ev.end_sample]
                        on_segment_end(seg_pcm, ev.start_sample / 16000,
                                       ev.end_sample / 16000)
            del base
        # 流结束，强制闭合未完结句段
        ev = session.flush()
        if ev is not None:
            segments.append((ev.start_sample, ev.end_sample))
            dur = (ev.end_sample - ev.start_sample) / 16000
            print(f"[vad] Sentence Detected (flush): {ev.start_sample / 16000:.2f}s"
                  f" ~ {ev.end_sample / 16000:.2f}s (时长 {dur:.2f}s)", flush=True)
            if on_segment_end is not None:
                full = np.concatenate(captured)
                on_segment_end(full[ev.start_sample:ev.end_sample],
                               ev.start_sample / 16000, ev.end_sample / 16000)
    finally:
        src.close()
    captured_pcm = np.concatenate(captured) if captured else np.zeros(0, np.int16)
    if args.save_capture:
        write_wav_16k(args.save_capture, captured_pcm)
        print(f"[save] 采集音频已保存: {args.save_capture}", flush=True)
    return captured_pcm, segments


def configure_console():
    """Windows 控制台 UTF-8 输出，避免中文乱码。"""
    try:
        sys.stdout.reconfigure(encoding="utf-8", errors="replace")
        sys.stderr.reconfigure(encoding="utf-8", errors="replace")
    except Exception:
        pass
