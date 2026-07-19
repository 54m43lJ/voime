"""POC-01: 加载三个模型（FireRedStreamVad / FireRedASR2-AED / FireRedPunc）。

验收标准：模型加载成功，无报错。
"""
import argparse
import time

from voime_poc import paths
from voime_poc.cli import configure_console
from voime_poc.engines import load_asr, load_punc, load_stream_vad


def main() -> int:
    configure_console()
    parser = argparse.ArgumentParser(description="POC-01 模型加载验证")
    parser.add_argument("--skip-smoke", action="store_true", help="跳过加载后的冒烟推理")
    args = parser.parse_args()

    reports = []

    print("=" * 64)
    print("POC-01: 加载 FireRedStreamVad / FireRedASR2-AED / FireRedPunc")
    print("=" * 64)

    vad, rep = load_stream_vad()
    reports.append(rep)
    print(f"[OK] {rep.name:<18s} 加载耗时 {rep.elapsed_s:6.2f}s  <- {rep.model_dir}")

    asr, rep = load_asr()
    reports.append(rep)
    print(f"[OK] {rep.name:<18s} 加载耗时 {rep.elapsed_s:6.2f}s  <- {rep.model_dir}")

    punc, rep = load_punc()
    reports.append(rep)
    print(f"[OK] {rep.name:<18s} 加载耗时 {rep.elapsed_s:6.2f}s  <- {rep.model_dir}")

    if not args.skip_smoke:
        # 冒烟推理：确认三链路可用
        from voime_poc.audio import read_wav_16k
        from voime_poc.engines import punc_text, transcribe_pcm

        wav = paths.ASSETS_DIR / "hello_zh.wav"
        pcm = read_wav_16k(wav)
        _, vad_result = vad.detect_full(pcm)
        print(f"[smoke] VAD detect_full({wav.name}): {vad_result['timestamps']}")

        t0 = time.perf_counter()
        r = transcribe_pcm(asr, pcm)
        print(f"[smoke] ASR: '{r['text']}' (dur={r['dur_s']}s, rtf={r['rtf']}, "
              f"耗时 {time.perf_counter() - t0:.2f}s)")

        t0 = time.perf_counter()
        pt = punc_text(punc, r["text"])
        print(f"[smoke] Punc: '{r['text']}' -> '{pt}' (耗时 {time.perf_counter() - t0:.2f}s)")

    print("-" * 64)
    total = sum(r.elapsed_s for r in reports)
    print(f"[POC-01 PASS] 三个模型全部加载成功，总耗时 {total:.2f}s")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
