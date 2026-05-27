import asyncio
import io
import logging
import os
import shutil
import tempfile
import time
import wave
from pathlib import Path
from typing import AsyncIterator

import numpy as np

from .engine import ASREngine
from .vad import SileroVAD

logger = logging.getLogger(__name__)


class WhisperCppEngine(ASREngine):
    """Streaming ASR using whisper.cpp CLI binary.

    Supports Vulkan / CUDA / CPU backends depending on how whisper.cpp was compiled.
    For AMD GPUs, build whisper.cpp with: cmake -DWHISPER_VULKAN=1
    """

    def __init__(self):
        self._config: dict = {}
        self._sample_rate = 16000
        self._binary: str | None = None
        self._model_path: str | None = None
        self._threads = 4

        self._buffer: np.ndarray | None = None
        self._last_text = ""
        self._stable_text = ""
        self._stable_count = 0
        self._vad: SileroVAD | None = None
        self._chunk_interval = 0.5
        self._stable_threshold = 2

    def initialize(self, config: dict) -> None:
        self._config = config
        self._sample_rate = config.get("sample_rate", 16000)

        wc = config.get("whisper_cpp", {})
        self._binary = wc.get("binary", self._find_default_binary())
        self._model_path = wc.get("model_path", self._find_default_model())
        self._threads = wc.get("threads", 4)
        self._format = wc.get("format", "txt")  # txt or srt

        if self._binary is None or not os.path.isfile(self._binary):
            logger.warning("whisper-cli binary not found at: %s", self._binary)
        if self._model_path is None or not os.path.isfile(self._model_path):
            logger.warning("whisper model not found at: %s", self._model_path)

        self._chunk_interval = config.get("streaming", {}).get("chunk_interval", 0.5)
        self._stable_threshold = config.get("streaming", {}).get("stable_count", 2)
        self._vad = SileroVAD(
            threshold=config.get("vad", {}).get("threshold", 0.5),
            min_speech_duration=config.get("vad", {}).get("min_speech_duration", 0.3),
            silence_duration=config.get("vad", {}).get("silence_duration", 1.0),
            sample_rate=self._sample_rate,
        )

    @staticmethod
    def _find_default_binary() -> str | None:
        candidates = [
            "whisper-cli",          # $PATH
            "./whisper.cpp/build/bin/whisper-cli",
            "../whisper.cpp/build/bin/whisper-cli",
        ]
        for c in candidates:
            if shutil.which(c) or os.path.isfile(c):
                return c
        return shutil.which("whisper-cli") or "whisper-cli"

    @staticmethod
    def _find_default_model() -> str | None:
        candidates = [
            "models/ggml-base.bin",
            "models/ggml-small.bin",
            "./whisper.cpp/models/ggml-base.bin",
            "../whisper.cpp/models/ggml-base.bin",
        ]
        for c in candidates:
            if os.path.isfile(c):
                return c
        return None

    def reset(self) -> None:
        self._buffer = None
        self._last_text = ""
        self._stable_text = ""
        self._stable_count = 0
        if self._vad:
            self._vad.reset()

    def _check_ready(self) -> bool:
        return (self._binary and os.path.isfile(self._binary) and
                self._model_path and os.path.isfile(self._model_path))

    async def transcribe_stream(self, audio_chunks: AsyncIterator[bytes]) -> AsyncIterator[dict]:
        if not self._check_ready():
            yield {"type": "error", "message": "whisper.cpp binary or model not found"}
            return

        self.reset()
        last_transcribe_time = 0.0

        async for chunk in audio_chunks:
            audio_np = np.frombuffer(chunk, dtype=np.int16).astype(np.float32) / 32768.0

            if self._buffer is None:
                self._buffer = audio_np
            else:
                self._buffer = np.concatenate([self._buffer, audio_np])

            if self._vad is not None:
                vad_state = self._vad.process_frame(np.frombuffer(chunk, dtype=np.int16))
                if vad_state == "speech_end" and self._buffer is not None and len(self._buffer) > 0:
                    result = await self._transcribe_buffer(self._buffer, final=True)
                    self.reset()
                    yield result
                    return

            now = time.time()
            if (now - last_transcribe_time >= self._chunk_interval and
                    self._buffer is not None and len(self._buffer) > self._sample_rate * 0.5):
                last_transcribe_time = now
                async for r in self._incremental_transcribe():
                    yield r

        if self._buffer is not None and len(self._buffer) > 0:
            result = await self._transcribe_buffer(self._buffer, final=True)
            self.reset()
            yield result

    async def _incremental_transcribe(self) -> AsyncIterator[dict]:
        result = await self._transcribe_buffer(self._buffer, final=False)
        if result["type"] == "final":
            return

        text = result.get("text", "")
        if text == self._last_text:
            self._stable_count += 1
            if self._stable_count >= self._stable_threshold and text != self._stable_text:
                self._stable_text = text
                yield {"type": "partial", "text": text, "stable": True}
            return

        self._last_text = text
        self._stable_count = 0

        if text.startswith(self._stable_text):
            new_part = text[len(self._stable_text):].strip()
            if new_part:
                yield {"type": "partial", "text": self._stable_text + new_part, "stable": False}
        else:
            yield {"type": "partial", "text": text, "stable": False}

    async def _transcribe_buffer(self, audio: np.ndarray, final: bool = False) -> dict:
        min_samples = int(self._sample_rate * 0.3)
        if len(audio) < min_samples:
            return {"type": "partial", "text": ""}

        if self._binary is None or self._model_path is None:
            return {"type": "error", "message": "Not initialized"}

        language = self._config.get("language", "zh")
        if language == "auto":
            language = "auto"

        wav_path = None
        try:
            # Write audio buffer to temp WAV
            fd, wav_path = tempfile.mkstemp(suffix=".wav", prefix="voime_")
            os.close(fd)

            self._write_wav(wav_path, audio, self._sample_rate)

            # Build whisper-cli command
            cmd = [
                self._binary,
                "-m", self._model_path,
                "-f", wav_path,
                "-l", language,
                "-t", str(self._threads),
                "-otxt",                    # text output
                "--print-progress", "0",    # suppress progress output
                "--no-timestamps",          # text only, no timestamps
            ]

            proc = await asyncio.create_subprocess_exec(
                *cmd,
                stdout=asyncio.subprocess.PIPE,
                stderr=asyncio.subprocess.PIPE,
            )
            stdout, stderr = await asyncio.wait_for(
                proc.communicate(), timeout=10.0
            )

            output = stdout.decode("utf-8", errors="replace").strip()

            # whisper-cli -otxt outputs each line as one segment
            # Remove leading bracketed markers like "[00:00:00.000 --> 00:00:02.000]"
            lines = []
            for line in output.split("\n"):
                line = line.strip()
                if not line:
                    continue
                # skip timestamp lines
                if line.startswith("[") and "-->" in line:
                    continue
                lines.append(line)

            text = " ".join(lines).strip()
            return {"type": "final" if final else "partial", "text": text}

        except asyncio.TimeoutError:
            logger.error("whisper-cli timed out")
            return {"type": "error", "message": "Transcription timed out"}
        except FileNotFoundError:
            logger.error("whisper-cli binary not found at: %s", self._binary)
            return {"type": "error", "message": "whisper-cli binary not found"}
        except Exception:
            logger.exception("whisper.cpp transcription failed")
            return {"type": "error", "message": "Transcription failed"}
        finally:
            if wav_path and os.path.exists(wav_path):
                try:
                    os.unlink(wav_path)
                except OSError:
                    pass

    @staticmethod
    def _write_wav(path: str, audio: np.ndarray, sample_rate: int) -> None:
        """Write float32 [-1,1] numpy array as 16-bit PCM WAV."""
        pcm = (audio * 32767.0).clip(-32768, 32767).astype(np.int16)
        with wave.open(path, "wb") as wf:
            wf.setnchannels(1)
            wf.setsampwidth(2)  # 16-bit
            wf.setframerate(sample_rate)
            wf.writeframes(pcm.tobytes())
