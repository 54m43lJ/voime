"""音频采集与处理：麦克风实时流 / 文件模拟流 / 重采样。"""
import queue
import time

import numpy as np
import soundfile as sf
import torch
import torchaudio

SAMPLE_RATE = 16000
FRAME_SAMPLES = 400  # 25ms @16kHz，与 FireRedStreamVad.detect_frame 对齐


def read_wav_16k(path) -> np.ndarray:
    """读取 wav -> int16 mono 16kHz（必要时重采样/混单声道）。"""
    data, sr = sf.read(str(path), dtype="int16", always_2d=True)
    if data.shape[1] > 1:
        data = data.mean(axis=1)
    else:
        data = data[:, 0]
    if sr != SAMPLE_RATE:
        f = torch.from_numpy(data.astype(np.float32))
        f = torchaudio.functional.resample(f, sr, SAMPLE_RATE)
        data = np.clip(f.numpy(), -32768, 32767).astype(np.int16)
    return data


def write_wav_16k(path, pcm_i16: np.ndarray) -> None:
    sf.write(str(path), pcm_i16.astype(np.int16), SAMPLE_RATE)


def float_to_i16(x: np.ndarray) -> np.ndarray:
    return np.clip(x * 32768.0, -32768, 32767).astype(np.int16)


class FrameSlicer:
    """把任意长度的 int16 PCM 流切成 400 样本（25ms）定长帧。"""

    def __init__(self, frame: int = FRAME_SAMPLES):
        self.frame = frame
        self._buf = np.zeros(0, dtype=np.int16)

    def feed(self, pcm: np.ndarray):
        self._buf = np.concatenate([self._buf, pcm.astype(np.int16)])
        n = len(self._buf) // self.frame * self.frame
        out = [self._buf[i:i + self.frame] for i in range(0, n, self.frame)]
        self._buf = self._buf[n:]
        return out


class MicSource:
    """默认输入设备的实时麦克风流，输出 16kHz int16 PCM。"""

    def __init__(self, block_ms: int = 100):
        import sounddevice as sd

        self._sd = sd
        dev = sd.default.device[0]
        info = sd.query_devices(dev, "input")
        self.device_name = info["name"]
        self.device_sr = int(info["default_samplerate"])
        self.block = int(self.device_sr * block_ms / 1000)
        self._q: queue.Queue = queue.Queue()
        self._stream = None

    def _callback(self, indata, frames, time_info, status):
        self._q.put(indata[:, 0].copy())

    def start(self):
        self._stream = self._sd.InputStream(
            samplerate=self.device_sr, channels=1, dtype="float32",
            blocksize=self.block, callback=self._callback)
        self._stream.start()
        return self

    def read(self, min_samples: int = 1600, timeout: float = 2.0) -> np.ndarray:
        """取至少 min_samples 个 16kHz int16 样本；超时返回已积攒的（可能为空）。"""
        deadline = time.perf_counter() + timeout
        acc = []
        got = 0
        while got < min_samples and time.perf_counter() < deadline:
            try:
                blk = self._q.get(timeout=max(0.0, deadline - time.perf_counter()))
            except queue.Empty:
                break
            blk16k = torchaudio.functional.resample(
                torch.from_numpy(blk), self.device_sr, SAMPLE_RATE).numpy()
            acc.append(blk16k)
            got += len(blk16k)
        if not acc:
            return np.zeros(0, dtype=np.float32)
        return np.concatenate(acc)

    def close(self):
        if self._stream is not None:
            self._stream.stop()
            self._stream.close()
            self._stream = None


class WavSource:
    """文件模拟实时流：按真实时间节奏喂 16kHz int16 PCM。"""

    def __init__(self, pcm_i16: np.ndarray, realtime: bool = True, chunk: int = 1600):
        self.pcm = pcm_i16.astype(np.int16)
        self.realtime = realtime
        self.chunk = chunk
        self._pos = 0
        self._t0 = None

    def start(self):
        self._t0 = time.perf_counter()
        return self

    def read(self, min_samples: int = 1600, timeout: float = 2.0) -> np.ndarray:
        del min_samples, timeout
        if self._pos >= len(self.pcm):
            return np.zeros(0, dtype=np.float32)
        if self.realtime:
            target = (self._pos + self.chunk) / SAMPLE_RATE
            delay = self._t0 + target - time.perf_counter()
            if delay > 0:
                time.sleep(delay)
        end = min(self._pos + self.chunk, len(self.pcm))
        out = self.pcm[self._pos:end].astype(np.float32) / 32768.0
        self._pos = end
        return out

    @property
    def exhausted(self) -> bool:
        return self._pos >= len(self.pcm)

    def close(self):
        pass
