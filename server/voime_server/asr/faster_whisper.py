import io
import logging
import time
from typing import AsyncIterator

import numpy as np
from faster_whisper import WhisperModel

from .engine import ASREngine
from .vad import SileroVAD

logger = logging.getLogger(__name__)


class FasterWhisperEngine(ASREngine):
    """Streaming ASR using faster-whisper with sliding window incremental decode."""

    def __init__(self):
        self._model: WhisperModel | None = None
        self._config: dict = {}
        self._sample_rate = 16000

        # per-utterance state
        self._buffer: np.ndarray | None = None
        self._last_text = ""
        self._stable_text = ""
        self._stable_count = 0
        self._vad: SileroVAD | None = None
        self._chunk_interval = 0.5
        self._stable_threshold = 2

    def initialize(self, config: dict) -> None:
        self._config = config
        self._model = WhisperModel(
            config.get("model", "base"),
            device=config.get("device", "cpu"),
            compute_type=config.get("compute_type", "int8"),
        )
        self._sample_rate = config.get("sample_rate", 16000)
        self._chunk_interval = config.get("streaming", {}).get("chunk_interval", 0.5)
        self._stable_threshold = config.get("streaming", {}).get("stable_count", 2)
        self._vad = SileroVAD(
            threshold=config.get("vad", {}).get("threshold", 0.5),
            min_speech_duration=config.get("vad", {}).get("min_speech_duration", 0.3),
            silence_duration=config.get("vad", {}).get("silence_duration", 1.0),
            sample_rate=self._sample_rate,
        )

    def reset(self) -> None:
        self._buffer = None
        self._last_text = ""
        self._stable_text = ""
        self._stable_count = 0
        if self._vad:
            self._vad.reset()

    async def transcribe_stream(self, audio_chunks: AsyncIterator[bytes]) -> AsyncIterator[dict]:
        """Process streaming audio chunks and yield partial/final results."""
        if self._model is None:
            yield {"type": "error", "message": "Engine not initialized"}
            return

        self.reset()
        # Timestamp of the last transcription pass
        last_transcribe_time = 0.0

        async for chunk in audio_chunks:
            audio_np = np.frombuffer(chunk, dtype=np.int16).astype(np.float32) / 32768.0

            # accumulate
            if self._buffer is None:
                self._buffer = audio_np
            else:
                self._buffer = np.concatenate([self._buffer, audio_np])

            # VAD check
            if self._vad is not None:
                vad_state = self._vad.process_frame(np.frombuffer(chunk, dtype=np.int16))
                if vad_state == "speech_end" and len(self._buffer) > 0:
                    result = await self._transcribe_buffer(self._buffer, final=True)
                    self.reset()
                    yield result
                    return

            # Incremental transcription
            now = time.time()
            if now - last_transcribe_time >= self._chunk_interval and len(self._buffer) > self._sample_rate * 0.5:
                last_transcribe_time = now
                async for r in self._incremental_transcribe():
                    yield r

        # End of stream
        if self._buffer is not None and len(self._buffer) > 0:
            result = await self._transcribe_buffer(self._buffer, final=True)
            self.reset()
            yield result

    async def _incremental_transcribe(self) -> AsyncIterator[dict]:
        """Run partial transcription and yield diffs."""
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

        # Compute diff: only yield new/changed portion
        if text.startswith(self._stable_text):
            new_part = text[len(self._stable_text):].strip()
            if new_part:
                yield {"type": "partial", "text": self._stable_text + new_part, "stable": False}
        else:
            # Full text changed, send all
            yield {"type": "partial", "text": text, "stable": False}

    async def _transcribe_buffer(self, audio: np.ndarray, final: bool = False) -> dict:
        """Run transcription on accumulated audio buffer."""
        if self._model is None or len(audio) < self._sample_rate * 0.3:
            return {"type": "partial", "text": ""}

        language = self._config.get("language")
        if language == "auto":
            language = None

        try:
            segments, info = self._model.transcribe(audio, language=language, beam_size=5)
            text = " ".join(seg.text.strip() for seg in segments)
            return {"type": "final" if final else "partial", "text": text}
        except Exception:
            logger.exception("Transcription failed")
            return {"type": "error", "message": "Transcription failed"}
