import asyncio
import json
import logging
from typing import AsyncIterator

from aiohttp import web

from .asr.engine import ASREngine
from .llm.engine import LLMEngine

logger = logging.getLogger(__name__)

_AUDIO_END = object()


class WebSocketHandler:
    def __init__(self, asr_engine: ASREngine, llm_engine: LLMEngine | None, config: dict):
        self.asr = asr_engine
        self.llm = llm_engine
        self.config = config

    async def handle_connection(self, request: web.Request) -> web.WebSocketResponse:
        ws = web.WebSocketResponse(max_msg_size=1024 * 1024)
        await ws.prepare(request)
        logger.info("Client connected: %s", request.remote)

        asr_config = dict(self.config.get("asr", {}))
        llm_enabled = self.config.get("llm", {}).get("enabled", False)
        llm_config = self.config.get("llm", {}) if llm_enabled else {}

        audio_queue: asyncio.Queue = asyncio.Queue()
        transcribe_task: asyncio.Task | None = None

        async def _audio_producer() -> AsyncIterator[bytes]:
            """Read from the asyncio.Queue, yield audio chunks until sentinel."""
            while True:
                item = await audio_queue.get()
                if item is _AUDIO_END:
                    return
                yield item

        async def _transcribe_and_push(final: bool = False):
            """Consume audio from the queue and push results to ws."""
            try:
                async for result in self.asr.transcribe_stream(_audio_producer()):
                    await ws.send_json(result)
                    if result["type"] == "final" and self.llm and llm_config:
                        text = result.get("text", "")
                        if text.strip():
                            polished = await self.llm.polish(
                                text,
                                llm_config.get("system_prompt", ""),
                                llm_config.get("user_prompt_template", ""),
                            )
                            await ws.send_json({"type": "polished", "text": polished})
            except Exception:
                logger.exception("Transcription error")

        try:
            async for msg in ws:
                if msg.type == web.WSMsgType.TEXT:
                    data = json.loads(msg.data)
                    msg_type = data.get("type")

                    if msg_type == "config":
                        asr_config.update(data)
                        self.asr.initialize(asr_config)
                        await ws.send_json({"type": "status", "code": "ready"})

                    elif msg_type == "speech_start":
                        self.asr.reset()
                        audio_queue = asyncio.Queue()
                        transcribe_task = asyncio.create_task(_transcribe_and_push())
                        await ws.send_json({"type": "status", "code": "listening"})

                    elif msg_type == "speech_end":
                        await audio_queue.put(_AUDIO_END)
                        if transcribe_task:
                            await transcribe_task
                            transcribe_task = None
                        await ws.send_json({"type": "status", "code": "done"})

                    elif msg_type == "polish":
                        if self.llm and llm_enabled:
                            text = data.get("text", "")
                            polished = await self.llm.polish(
                                text,
                                llm_config.get("system_prompt", ""),
                                llm_config.get("user_prompt_template", ""),
                            )
                            await ws.send_json({"type": "polished", "text": polished})
                        else:
                            await ws.send_json({"type": "polished", "text": data.get("text", "")})

                elif msg.type == web.WSMsgType.BINARY:
                    await audio_queue.put(msg.data)

        except Exception:
            logger.exception("WebSocket connection error")
        finally:
            if transcribe_task and not transcribe_task.done():
                transcribe_task.cancel()
            logger.info("Client disconnected: %s", request.remote)
        return ws
