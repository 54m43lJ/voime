#!/usr/bin/env bash
set -euo pipefail

# One-time ASR v2 migration helper. This script is deliberately not called by
# the installers; clean install remains the default upgrade path.

apply=0
force=0
gpu=0
model_root="${XDG_DATA_HOME:-$HOME/.local/share}/voime/models"
config_dir="${XDG_CONFIG_HOME:-$HOME/.config}"

usage() {
  cat >&2 <<'EOF'
usage: migrate-asr-v2.sh [--dry-run] [--apply] [--force]
                         [--gpu]
                         [--model-root DIR]

Without --apply the script only prints the planned actions.
EOF
  exit 2
}

while [ $# -gt 0 ]; do
  case "$1" in
    --dry-run) apply=0; shift ;;
    --apply) apply=1; shift ;;
    --force) force=1; shift ;;
    --gpu) gpu=1; shift ;;
    --model-root) model_root="${2:-}"; shift 2 ;;
    -h|--help) usage ;;
    *) echo "migrate-asr-v2: unknown argument: $1" >&2; usage ;;
  esac
done

marker="$model_root/.migrate-asr-v2"
if [ -f "$marker" ] && [ "$force" -eq 0 ]; then
  echo "migrate-asr-v2: already migrated ($marker); use --force to repeat" >&2
  exit 1
fi

if [ "$apply" -eq 1 ] && pgrep -x voimed >/dev/null 2>&1; then
  echo "migrate-asr-v2: 请先停止 voimed" >&2
  exit 1
fi

timestamp="$(date -u +%Y%m%dT%H%M%SZ)"
backup="$model_root.backup-$timestamp"
echo "gpu:        $gpu"
echo "model root: $model_root"
echo "backup:     $backup"
echo "config:     $config_dir/fcitx5/conf/voime.conf"

if [ "$apply" -eq 0 ]; then
  echo "dry-run: 将备份旧模型目录，安装 VAD + SenseVoice（可选 GPU），并写迁移标记"
  exit 0
fi

mkdir -p "$(dirname "$backup")"
if [ -d "$model_root" ]; then
  cp -a "$model_root" "$backup"
fi
if [ -f "$config_dir/fcitx5/conf/voime.conf" ]; then
  mkdir -p "$backup/config"
  cp -a "$config_dir/fcitx5/conf/voime.conf" "$backup/config/voime.conf"
fi

install_args=(--model-root "$model_root")
if [ "$gpu" -eq 1 ]; then
  install_args+=(--gpu)
fi
"$(dirname "${BASH_SOURCE[0]}")/install_models.sh" "${install_args[@]}"

printf '%s\n' "$timestamp" >"$marker"
echo "migrate-asr-v2: 完成；备份位于 $backup"
