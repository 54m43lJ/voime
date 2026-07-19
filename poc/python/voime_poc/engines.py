"""模型加载封装：FireRedStreamVad / FireRedASR2-AED / FireRedPunc。

统一 CPU 推理（POC 目标机无独立显卡要求，且产品目标是 CPU+AVX2）。
"""
import time
from dataclasses import dataclass

import numpy as np

from . import paths

paths.setup_sys_path()


@dataclass
class LoadReport:
    name: str
    model_dir: str
    elapsed_s: float


def load_stream_vad(use_gpu: bool = False, speech_threshold: float = 0.4):
    from fireredasr2s.fireredvad import FireRedStreamVad, FireRedStreamVadConfig

    cfg = FireRedStreamVadConfig(
        use_gpu=use_gpu,
        smooth_window_size=5,
        speech_threshold=speech_threshold,
        pad_start_frame=5,
        min_speech_frame=8,
        max_speech_frame=2000,
        min_silence_frame=20,
        chunk_max_frame=30000,
    )
    t0 = time.perf_counter()
    vad = FireRedStreamVad.from_pretrained(str(paths.VAD_STREAM_MODEL_DIR), cfg)
    return vad, LoadReport("FireRedStreamVad", str(paths.VAD_STREAM_MODEL_DIR),
                           time.perf_counter() - t0)


def load_asr(use_gpu: bool = False, beam_size: int = 3, return_timestamp: bool = False):
    from fireredasr2s.fireredasr2 import FireRedAsr2, FireRedAsr2Config

    cfg = FireRedAsr2Config(
        use_gpu=use_gpu,
        use_half=False,
        beam_size=beam_size,
        nbest=1,
        decode_max_len=0,
        softmax_smoothing=1.25,
        aed_length_penalty=0.6,
        eos_penalty=1.0,
        return_timestamp=return_timestamp,
    )
    t0 = time.perf_counter()
    asr = FireRedAsr2.from_pretrained("aed", str(paths.ASR_MODEL_DIR), cfg)
    return asr, LoadReport("FireRedASR2-AED", str(paths.ASR_MODEL_DIR),
                           time.perf_counter() - t0)


def load_punc(use_gpu: bool = False):
    from fireredasr2s.fireredpunc import FireRedPunc, FireRedPuncConfig

    cfg = FireRedPuncConfig(use_gpu=use_gpu)
    t0 = time.perf_counter()
    punc = FireRedPunc.from_pretrained(str(paths.PUNC_MODEL_DIR), cfg)
    return punc, LoadReport("FireRedPunc", str(paths.PUNC_MODEL_DIR),
                            time.perf_counter() - t0)


def transcribe_pcm(asr, pcm_i16: np.ndarray, uttid: str = "seg") -> dict:
    """直接对内存中的 16kHz int16 PCM 做识别（绕过文件 IO）。"""
    results = asr.transcribe([uttid], [(16000, pcm_i16)])
    return results[0]


def punc_text(punc, text: str) -> str:
    results = punc.process([text])
    return results[0]["punc_text"] if results else ""
