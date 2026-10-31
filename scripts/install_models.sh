#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PYTHON="${VOIME_PYTHON:-python3}"
CACHE="${XDG_CACHE_HOME:-$HOME/.cache}/voime"
VENV="$CACHE/venv"
REQUIREMENTS="$ROOT/tools/requirements-export.txt"

# SenseVoice only needs the Python standard library. Avoid creating the heavy
# PyTorch export environment when it is not needed.
ASR="fire-red-ctc"
PREVIOUS=""
for ARG in "$@"; do
  case "$ARG" in
    --asr=sense-voice) ASR="sense-voice" ;;
    --asr=fire-red-aed) ASR="fire-red-aed" ;;
    sense-voice|fire-red-aed)
      if [ "$PREVIOUS" = "--asr" ]; then ASR="$ARG"; fi
      ;;
  esac
  PREVIOUS="$ARG"
done
if [ "$ASR" = "sense-voice" ] || [ "$ASR" = "fire-red-aed" ]; then
  exec "$PYTHON" "$ROOT/tools/install_models.py" "$@"
fi

if ! "$PYTHON" -c 'import sys; raise SystemExit(0 if sys.version_info >= (3, 10) else 1)'; then
  echo "install_models: 需要 Python 3.10+" >&2
  exit 1
fi

mkdir -p "$CACHE"
if [ ! -x "$VENV/bin/python" ]; then
  echo "install_models: 创建虚拟环境 $VENV"
  "$PYTHON" -m venv "$VENV"
fi

REQ_HASH="$(sha256sum "$REQUIREMENTS" | cut -d' ' -f1)"
MARKER="$VENV/.voime-requirements"
if [ ! -f "$MARKER" ] || [ "$(cat "$MARKER")" != "$REQ_HASH" ]; then
  echo "install_models: 安装导出依赖"
  "$VENV/bin/python" -m pip install --upgrade pip
  "$VENV/bin/python" -m pip install -r "$REQUIREMENTS"
  printf '%s\n' "$REQ_HASH" >"$MARKER"
fi

exec "$VENV/bin/python" "$ROOT/tools/install_models.py" "$@"
