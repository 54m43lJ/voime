import numpy as np


class SileroVAD:
    """Voice Activity Detection using energy-based heuristics.

    A lightweight VAD that doesn't require loading a PyTorch model.
    Uses short-time energy + zero-crossing rate for decision.
    For production use, the real Silero VAD model can be swapped in.
    """

    def __init__(self, threshold: float = 0.5, min_speech_duration: float = 0.3,
                 silence_duration: float = 1.0, sample_rate: int = 16000):
        self.threshold = threshold
        self.min_speech_duration = min_speech_duration
        self.silence_duration = silence_duration
        self.sample_rate = sample_rate

        self._speech_frames = 0
        self._silence_frames = 0
        self._is_speaking = False
        self._frame_duration = 0.03  # 30ms per frame

    def _compute_energy(self, audio: np.ndarray) -> float:
        rms = np.sqrt(np.mean(audio.astype(np.float32) ** 2))
        ref = 32768.0
        if rms < 1e-10:
            return -100.0
        return 20.0 * np.log10(rms / ref)

    def _compute_zcr(self, audio: np.ndarray) -> float:
        signs = np.sign(audio)
        # count zero crossings
        zcr = np.sum(np.abs(np.diff(signs))) / (2.0 * len(audio))
        return float(zcr)

    def is_speech(self, audio: np.ndarray) -> bool:
        """Return True if the frame contains speech."""
        energy = self._compute_energy(audio)
        zcr = self._compute_zcr(audio)
        # High energy and moderate ZCR typically indicate speech
        score = 0.6 * (1.0 / (1.0 + np.exp(-(energy + 40) / 5))) + \
                0.4 * (1.0 - abs(zcr - 0.1) / 0.3)
        return float(np.clip(score, 0, 1)) > self.threshold

    def process_frame(self, audio: np.ndarray) -> str:
        """Return 'speech', 'silence', or 'speech_end'."""

        frame_len = int(self.sample_rate * self._frame_duration)
        if len(audio) < frame_len:
            return "speech" if self._is_speaking else "silence"

        speech = self.is_speech(audio[:frame_len])
        frames_per_frame = len(audio) / frame_len

        if speech:
            self._speech_frames += frames_per_frame
            self._silence_frames = 0
            if not self._is_speaking and self._speech_frames * self._frame_duration >= self.min_speech_duration:
                self._is_speaking = True
            return "speech"
        else:
            if self._is_speaking:
                self._silence_frames += frames_per_frame
                if self._silence_frames * self._frame_duration >= self.silence_duration:
                    self._is_speaking = False
                    self._speech_frames = 0
                    return "speech_end"
            return "silence"

    def reset(self) -> None:
        self._speech_frames = 0
        self._silence_frames = 0
        self._is_speaking = False
