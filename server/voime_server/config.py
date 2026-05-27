import os
import re
from pathlib import Path
from typing import Any

import yaml


DEFAULT_CONFIG = {
    "server": {"host": "127.0.0.1", "port": 9876},
    "asr": {
        "backend": "faster_whisper",
        "model": "base",
        "language": "zh",
        "device": "cpu",
        "compute_type": "int8",
        "vad": {
            "threshold": 0.5,
            "min_speech_duration": 0.3,
            "silence_duration": 1.0,
        },
        "streaming": {"chunk_interval": 0.5, "stable_count": 2},
        "openai": {"api_key": "", "base_url": "https://api.openai.com/v1"},
        "whisper_cpp": {
            "binary": "",           # path to whisper-cli (auto-detect if empty)
            "model_path": "",       # path to ggml model (auto-detect if empty)
            "threads": 4,
            "format": "txt",
        },
    },
    "llm": {
        "enabled": False,
        "backend": "openai_compat",
        "base_url": "http://127.0.0.1:11434/v1",
        "api_key": "ollama",
        "model": "qwen3:4b",
        "timeout": 15,
        "system_prompt": "你是一个专业的文本校对助手，只修正错别字、听错词和标点符号，不改变原意和语气，不添加额外内容。",
        "user_prompt_template": "请修正以下语音输入文本：\n{text}",
    },
}


def _resolve_env_vars(value: Any) -> Any:
    if isinstance(value, str):
        pattern = re.compile(r"\$\{(\w+)\}")
        matches = pattern.findall(value)
        if len(matches) == 1 and value.strip() == f"${{{matches[0]}}}":
            return os.environ.get(matches[0], value)
        return pattern.sub(lambda m: os.environ.get(m.group(1), m.group(0)), value)
    if isinstance(value, dict):
        return {k: _resolve_env_vars(v) for k, v in value.items()}
    if isinstance(value, list):
        return [_resolve_env_vars(v) for v in value]
    return value


def _deep_merge(a: dict, b: dict) -> dict:
    result = dict(a)
    for k, v in b.items():
        if k in result and isinstance(result[k], dict) and isinstance(v, dict):
            result[k] = _deep_merge(result[k], v)
        else:
            result[k] = v
    return result


def find_config() -> Path | None:
    """Search config paths in priority order."""
    paths = [
        Path(os.environ.get("VOIME_CONFIG", "")),
        Path.home() / ".config" / "voime" / "config.yaml",
        Path("config.yaml"),
    ]
    for p in paths:
        if p.is_file():
            return p
    return None


def load_config(path: str | Path | None = None) -> dict:
    if path is None:
        path = find_config()

    if path and Path(path).is_file():
        with open(path, encoding="utf-8") as f:
            user_config = yaml.safe_load(f) or {}
    else:
        user_config = {}

    config = _deep_merge(DEFAULT_CONFIG, user_config)
    return _resolve_env_vars(config)
