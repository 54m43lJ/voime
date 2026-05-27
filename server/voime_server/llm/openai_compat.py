import logging

import httpx

from .engine import LLMEngine

logger = logging.getLogger(__name__)


class OpenAICompatEngine(LLMEngine):
    def __init__(self, config: dict):
        self._base_url = config.get("base_url", "").rstrip("/")
        self._api_key = config.get("api_key", "")
        self._model = config.get("model", "")
        self._timeout = config.get("timeout", 15)

    async def polish(self, text: str, system_prompt: str, user_prompt_template: str) -> str:
        if not text.strip():
            return text

        user_prompt = user_prompt_template.format(text=text)
        headers = {
            "Authorization": f"Bearer {self._api_key}",
            "Content-Type": "application/json",
        }
        body = {
            "model": self._model,
            "messages": [
                {"role": "system", "content": system_prompt},
                {"role": "user", "content": user_prompt},
            ],
            "temperature": 0.3,
            "max_tokens": len(text) * 2 + 200,
        }

        try:
            async with httpx.AsyncClient(timeout=self._timeout) as client:
                resp = await client.post(
                    f"{self._base_url}/chat/completions",
                    headers=headers,
                    json=body,
                )
                resp.raise_for_status()
                data = resp.json()
                polished = data["choices"][0]["message"]["content"].strip()
                return polished
        except Exception:
            logger.exception("LLM polishing failed, returning original text")
            return text
