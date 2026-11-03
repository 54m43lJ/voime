#!/usr/bin/env bash
set -euo pipefail

# One-time ASR v2 migration helper. This script is deliberately not called by
# the installers; clean install remains the default upgrade path.

apply=0
force=0
gpu=0
model_root="${XDG_DATA_HOME:-$HOME/.local/share}/voime/models"
config_dir="${XDG_CONFIG_HOME:-$HOME/.config}"
cache_dir=""
source_dir=""
punc_source_dir=""
aed_version=""
backup=""
started=0

usage() {
  cat >&2 <<'EOF'
usage: migrate-asr-v2.sh [--dry-run] [--apply] [--force]
                         [--gpu]
                         [--model-root DIR]
                         [--cache-dir DIR]
                         [--source-dir DIR]
                         [--punc-source-dir DIR]
                         [--aed-version v1|v2|auto]

Without --apply the script only prints the planned actions.
EOF
  exit 2
}

restore_on_error() {
  local status=$?
  if [ "$status" -ne 0 ] && [ "$started" -eq 1 ]; then
    echo "migrate-asr-v2: 安装失败，正在恢复迁移前模型目录" >&2
    rm -rf "$model_root"
    if [ -d "$model_backup" ]; then
      mkdir -p "$model_root"
      cp -a "$model_backup/." "$model_root/"
      echo "migrate-asr-v2: 已恢复 $model_root" >&2
    fi
  fi
  exit "$status"
}

need_value() {
  [ "$#" -ge 2 ] || {
    echo "migrate-asr-v2: $1 需要参数" >&2
    usage
  }
}

while [ $# -gt 0 ]; do
  case "$1" in
    --dry-run) apply=0; shift ;;
    --apply) apply=1; shift ;;
    --force) force=1; shift ;;
    --gpu) gpu=1; shift ;;
    --model-root)
      need_value "$@"
      model_root="$2"
      shift 2
      ;;
    --cache-dir)
      need_value "$@"
      cache_dir="$2"
      shift 2
      ;;
    --source-dir)
      need_value "$@"
      source_dir="$2"
      shift 2
      ;;
    --punc-source-dir)
      need_value "$@"
      punc_source_dir="$2"
      shift 2
      ;;
    --aed-version)
      need_value "$@"
      aed_version="$2"
      shift 2
      ;;
    -h|--help) usage ;;
    *) echo "migrate-asr-v2: unknown argument: $1" >&2; usage ;;
  esac
done

marker="$model_root/.migrate-asr-v2"
if [ -z "$model_root" ] || [ "$model_root" = "/" ]; then
  echo "migrate-asr-v2: --model-root 不能为空或 /" >&2
  exit 2
fi
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
model_backup="$backup/models"
config_backup="$backup/config"
echo "gpu:        $gpu"
echo "model root: $model_root"
echo "backup:     $backup"
echo "config:     $config_dir/fcitx5/conf/voime.conf"

if [ "$apply" -eq 0 ]; then
  echo "dry-run: 将备份旧模型目录，安装 default/ 资源包（可选 firered/），清理旧根级模型目录，并写迁移标记"
  exit 0
fi

mkdir -p "$backup"
if [ -d "$model_root" ]; then
  mkdir -p "$model_backup"
  cp -a "$model_root/." "$model_backup/"
fi
if [ -f "$config_dir/fcitx5/conf/voime.conf" ]; then
  mkdir -p "$config_backup"
  cp -a "$config_dir/fcitx5/conf/voime.conf" "$config_backup/voime.conf"
fi

trap restore_on_error EXIT
started=1

install_args=(--model-root "$model_root")
if [ "$gpu" -eq 1 ]; then
  install_args+=(--gpu)
fi
if [ -n "$cache_dir" ]; then
  install_args+=(--cache-dir "$cache_dir")
fi
if [ -n "$source_dir" ]; then
  install_args+=(--source-dir "$source_dir")
fi
if [ -n "$punc_source_dir" ]; then
  install_args+=(--punc-source-dir "$punc_source_dir")
fi
if [ -n "$aed_version" ]; then
  install_args+=(--aed-version "$aed_version")
fi
"$(dirname "${BASH_SOURCE[0]}")/install_models.sh" "${install_args[@]}"

# Old root-level model directories are replaced by default/ and firered/.
for legacy in vad sense-voice fire-red-aed punc fire-red-aed-v1; do
  rm -rf "$model_root/$legacy"
done
rm -f "$model_root/active"

printf '%s\n' "$timestamp" >"$marker"
started=0
trap - EXIT
echo "migrate-asr-v2: 完成；备份位于 $backup"
