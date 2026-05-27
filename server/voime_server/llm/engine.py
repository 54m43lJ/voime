from abc import ABC, abstractmethod


class LLMEngine(ABC):
    @abstractmethod
    async def polish(self, text: str, system_prompt: str, user_prompt_template: str) -> str: ...
