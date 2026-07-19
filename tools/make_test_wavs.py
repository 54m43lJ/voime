"""生成 POC 测试音频：多句拼接（语音+静音交替），模拟一次 PTT 会话。"""
import sys
from pathlib import Path

import numpy as np
import soundfile as sf

ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(ROOT / "poc" / "python"))

from voime_poc import paths  # noqa: E402
from voime_poc.audio import read_wav_16k, write_wav_16k  # noqa: E402


def main():
    out = paths.ensure_out_dir()
    zh = read_wav_16k(paths.ASSETS_DIR / "hello_zh.wav")
    en = read_wav_16k(paths.ASSETS_DIR / "hello_en.wav")
    silence = np.zeros(16000, dtype=np.int16)  # 1s

    # 多句会话：3 句中文，句间 1s 静音（> min_silence_frame=20 帧=0.5s，必触发切分）
    sess = np.concatenate([zh, silence, zh, silence, zh])
    write_wav_16k(out / "session_zh_3sent.wav", sess)
    # 中英混合会话
    sess2 = np.concatenate([zh, silence, en, silence, zh])
    write_wav_16k(out / "session_mix_3sent.wav", sess2)
    # 长静音前缀（测试 VAD 不抖动）
    sess3 = np.concatenate([np.zeros(32000, dtype=np.int16), zh, silence, en])
    write_wav_16k(out / "session_lead_silence.wav", sess3)

    for f in sorted(out.glob("session_*.wav")):
        d, sr = sf.read(str(f), dtype="int16")
        print(f"{f.name}: {len(d) / sr:.2f}s")


if __name__ == "__main__":
    main()
