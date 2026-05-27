from abc import ABC, abstractmethod
from typing import AsyncIterator


class ASREngine(ABC):
    @abstractmethod
    def initialize(self, config: dict) -> None: ...

    @abstractmethod
    async def transcribe_stream(self, audio_chunks: AsyncIterator[bytes]) -> AsyncIterator[dict]:
        """Yield {"type": "partial"|"final", "text": "...", "stable": bool}."""
        ...

    @abstractmethod
    def reset(self) -> None:
        """Reset state for a new utterance."""
        ...
