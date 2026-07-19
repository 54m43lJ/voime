"""流式 VAD 会话：把 PCM 流切成语音句段（sample 级边界）。"""
from dataclasses import dataclass

import numpy as np

from . import paths
from .audio import FRAME_SAMPLES, FrameSlicer

paths.setup_sys_path()
from fireredasr2s.fireredvad.core.stream_vad_postprocessor import VadState  # noqa: E402


@dataclass
class SegmentEvent:
    """一次句子边界事件。kind: 'start' | 'end'；样本偏移为会话绝对坐标。"""
    kind: str
    start_sample: int
    end_sample: int


class VadSession:
    """包装 FireRedStreamVad，把 detect_frame 的帧索引换算成样本偏移。"""

    def __init__(self, vad):
        self.vad = vad
        self.slicer = FrameSlicer(FRAME_SAMPLES)
        self.frames_fed = 0  # 已喂给 VAD 的 25ms 帧数
        self.vad.reset()

    def push(self, pcm_i16: np.ndarray):
        """喂入任意长度 int16 PCM，返回本批产生的 SegmentEvent 列表。"""
        events = []
        for frame in self.slicer.feed(pcm_i16):
            r = self.vad.detect_frame(frame)
            self.frames_fed += 1
            if r.is_speech_start:
                events.append(SegmentEvent(
                    "start", (r.speech_start_frame - 1) * FRAME_SAMPLES, -1))
            if r.is_speech_end:
                events.append(SegmentEvent(
                    "end",
                    (r.speech_start_frame - 1) * FRAME_SAMPLES,
                    r.speech_end_frame * FRAME_SAMPLES))
        return events

    def flush(self):
        """流结束时若仍处于语音段，强制闭合当前句段。返回事件或 None。"""
        pp = self.vad.postprocessor
        if pp.state in (VadState.SPEECH, VadState.POSSIBLE_SILENCE) \
                and pp.last_speech_start_frame > 0:
            return SegmentEvent(
                "end",
                (pp.last_speech_start_frame - 1) * FRAME_SAMPLES,
                self.frames_fed * FRAME_SAMPLES)
        return None

    @property
    def fed_samples(self) -> int:
        return self.frames_fed * FRAME_SAMPLES
