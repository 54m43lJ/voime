"""POC-02: 麦克风实时录音 + FireRedStreamVad 句子切分。

验收标准：能够在说话时打印出 "Sentence Detected" 标记。
默认 wav 模拟流（自动化环境无人说话）；--source mic 走真实麦克风。
"""
import argparse

from voime_poc import paths
from voime_poc.cli import add_source_args, configure_console, run_vad_session
from voime_poc.engines import load_stream_vad


def main() -> int:
    configure_console()
    parser = argparse.ArgumentParser(description="POC-02 实时 VAD 句子切分")
    add_source_args(parser)
    args = parser.parse_args()

    print("=" * 64)
    print("POC-02: 实时录音 + 流式 VAD 切分")
    print("=" * 64)

    vad, rep = load_stream_vad(speech_threshold=args.threshold)
    print(f"[OK] {rep.name} 加载耗时 {rep.elapsed_s:.2f}s")

    if not args.save_capture:
        args.save_capture = str(paths.ensure_out_dir() / "poc02_capture.wav")
    captured, segments = run_vad_session(vad, args)

    print("-" * 64)
    print(f"[summary] 采集 {len(captured) / 16000:.2f}s，检出句子 {len(segments)} 段:")
    for i, (s, e) in enumerate(segments, 1):
        print(f"  #{i}: {s / 16000:.2f}s ~ {e / 16000:.2f}s ({(e - s) / 16000:.2f}s)")
    ok = len(segments) > 0
    print(f"[POC-02 {'PASS' if ok else 'FAIL'}] "
          f"{'成功检出语音句段' if ok else '未检出任何句段'}")
    return 0 if ok else 1


if __name__ == "__main__":
    raise SystemExit(main())
