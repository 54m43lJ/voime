#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PYTHON="${VOIME_PYTHON:-python3}"

if ! "$PYTHON" -c 'import sys; raise SystemExit(0 if sys.version_info >= (3, 10) else 1)'; then
  echo "install_models: 需要 Python 3.10+" >&2
  exit 1
fi

exec "$PYTHON" "$ROOT/tools/install_models.py" "$@"
