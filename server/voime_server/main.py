import argparse
import logging
import sys

from aiohttp import web

from .asr.engine import ASREngine
from .asr.faster_whisper import FasterWhisperEngine
from .asr.whisper_cpp import WhisperCppEngine
from .config import load_config
from .llm.openai_compat import OpenAICompatEngine
from .ws_handler import WebSocketHandler

logger = logging.getLogger(__name__)


def _create_asr_engine(config: dict) -> ASREngine:
    backend = config.get("asr", {}).get("backend", "faster_whisper")
    if backend == "whisper_cpp":
        return WhisperCppEngine()
    elif backend == "openai_realtime":
        raise NotImplementedError("OpenAI Realtime API backend not yet implemented")
    else:
        return FasterWhisperEngine()


def create_app(config: dict) -> web.Application:
    asr_engine = _create_asr_engine(config)
    llm_config = config.get("llm", {})
    llm_engine = OpenAICompatEngine(llm_config) if llm_config.get("enabled") else None

    handler = WebSocketHandler(asr_engine, llm_engine, config)
    app = web.Application()

    app.router.add_get("/asr", handler.handle_connection)

    async def health(_request: web.Request) -> web.Response:
        return web.json_response({"status": "ok"})

    app.router.add_get("/health", health)

    return app


def main():
    parser = argparse.ArgumentParser(description="Voime ASR Server")
    parser.add_argument("-c", "--config", help="Path to config file")
    parser.add_argument("-H", "--host", help="Listen host")
    parser.add_argument("-p", "--port", type=int, help="Listen port")
    parser.add_argument("--debug", action="store_true", help="Debug logging")
    args = parser.parse_args()

    logging.basicConfig(
        level=logging.DEBUG if args.debug else logging.INFO,
        format="%(asctime)s %(levelname)s %(name)s: %(message)s",
    )

    config = load_config(args.config)
    server_cfg = config["server"]
    host = args.host or server_cfg["host"]
    port = args.port or server_cfg["port"]

    logger.info("Starting Voime server on %s:%d", host, port)
    logger.info("ASR backend: %s (model: %s)", config["asr"]["backend"], config["asr"]["model"])
    if config["llm"]["enabled"]:
        logger.info("LLM polishing enabled: %s", config["llm"]["model"])
    else:
        logger.info("LLM polishing disabled")

    app = create_app(config)
    web.run_app(app, host=host, port=port)


if __name__ == "__main__":
    main()
