#!/usr/bin/env bash
# Voime GPU module install (Linux): FireRed and/or Qwen workers, runtimes, models.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PREFIX="${VOIME_PREFIX:-$HOME/.local}"
MODEL_ROOT="${VOIME_MODEL_ROOT:-${XDG_DATA_HOME:-$HOME/.local/share}/voime/models}"
MODEL_SOURCE=""
INSTALL_FIRERED=1
INSTALL_QWEN3=1
INSTALL_MODELS=1
UNINSTALL=0
INSTALL_DEPS=0
FIRERED_RUNTIME="${VOIME_FIRERED_RUNTIME:-}"
QWEN_RUNTIME="${VOIME_QWEN_RUNTIME:-vulkan}"
BACKEND=""
EXPLICIT_BACKEND=0
EXPLICIT_TARGET=0

usage() {
  cat <<'EOF'
Usage: scripts/install-gpu.sh [--firered|--qwen3] [options]

Options:
  --firered                       Install only the FireRed worker.
  --qwen3                         Install only the Qwen worker.
  --firered-runtime cuda|migraphx|directml
  --qwen-runtime vulkan|cuda
  --backend cpu|firered|qwen3      Write final.backend after installation.
  --no-models                     Install workers/runtimes only.
  --model-dir <dir>               Model root.
  --source-dir <dir>              Offline worker/runtime/model source.
  --prefix <dir>                  Install prefix (default: $HOME/.local).
  --install-deps                  Install missing Arch Linux packages with pacman.
  --uninstall                     Remove selected GPU backends.
  -h, --help                      Show this help.
EOF
}

while (($#)); do
  case "$1" in
    --firered) INSTALL_FIRERED=1; INSTALL_QWEN3=0; EXPLICIT_TARGET=1 ;;
    --qwen3) INSTALL_FIRERED=0; INSTALL_QWEN3=1; EXPLICIT_TARGET=1 ;;
    --firered-runtime) shift; FIRERED_RUNTIME="${1:?missing runtime}"; EXPLICIT_TARGET=1 ;;
    --qwen-runtime) shift; QWEN_RUNTIME="${1:?missing runtime}"; EXPLICIT_TARGET=1 ;;
    --backend) shift; BACKEND="${1:?missing backend}"; EXPLICIT_BACKEND=1 ;;
    --no-models) INSTALL_MODELS=0 ;;
    --model-dir) shift; MODEL_ROOT="${1:?missing --model-dir value}" ;;
    --source-dir) shift; MODEL_SOURCE="${1:?missing --source-dir value}" ;;
    --prefix) shift; PREFIX="${1:?missing --prefix value}" ;;
    --install-deps) INSTALL_DEPS=1 ;;
    --uninstall) UNINSTALL=1 ;;
    -h|--help) usage; exit 0 ;;
    *) echo "unknown option: $1" >&2; usage >&2; exit 2 ;;
  esac
  shift
done

BIN_DIR="$PREFIX/bin"

gpu_vendor() {
  local vendor_file vendor
  for vendor_file in /sys/class/drm/card*/device/vendor; do
    [[ -r "$vendor_file" ]] || continue
    read -r vendor < "$vendor_file" || continue
    case "$vendor" in
      0x1002) echo amd; return ;;
      0x10de) echo nvidia; return ;;
      0x8086) echo intel; return ;;
    esac
  done
  if command -v lspci >/dev/null 2>&1; then
    if lspci 2>/dev/null | grep -Eiq 'AMD|ATI'; then
      echo amd
    elif lspci 2>/dev/null | grep -Eiq 'NVIDIA'; then
      echo nvidia
    else
      echo unknown
    fi
  else
    echo unknown
  fi
}

GPU_VENDOR="$(gpu_vendor)"
if [[ -z "$FIRERED_RUNTIME" ]]; then
  if [[ "$GPU_VENDOR" == amd ]]; then
    FIRERED_RUNTIME=migraphx
  else
    FIRERED_RUNTIME=cuda
  fi
fi
if [[ "$FIRERED_RUNTIME" == directml ]]; then
  echo "--firered-runtime directml is Windows-only; on Linux use cuda or migraphx" >&2
  exit 2
fi
case "$FIRERED_RUNTIME" in cuda|migraphx) ;; *) echo "invalid --firered-runtime: $FIRERED_RUNTIME" >&2; exit 2 ;; esac
case "$QWEN_RUNTIME" in vulkan|cuda) ;; *) echo "invalid --qwen-runtime: $QWEN_RUNTIME" >&2; exit 2 ;; esac
if [[ -n "$BACKEND" ]]; then
  case "$BACKEND" in cpu|firered|qwen3) ;; *) echo "invalid --backend: $BACKEND" >&2; exit 2 ;; esac
  if ((EXPLICIT_TARGET)); then
    if [[ "$BACKEND" == firered && "$INSTALL_FIRERED" != 1 && "$INSTALL_QWEN3" == 1 ]]; then
      echo "--backend firered requires installing the FireRed backend" >&2
      exit 2
    fi
    if [[ "$BACKEND" == qwen3 && "$INSTALL_QWEN3" != 1 && "$INSTALL_FIRERED" == 1 ]]; then
      echo "--backend qwen3 requires installing the Qwen backend" >&2
      exit 2
    fi
  fi
fi
if [[ "$GPU_VENDOR" == amd && "$QWEN_RUNTIME" != vulkan ]]; then
  echo "AMD Linux only supports --qwen-runtime vulkan (no ROCm / HIP)" >&2
  exit 2
fi

if ((UNINSTALL)); then
  RUNTIME_DIR="${VOIME_RUNTIME_DIR:-$PREFIX/lib/voime}"
  if ((INSTALL_FIRERED)); then
    rm -f "$BIN_DIR/voime-gpu-firered"
    rm -rf "$RUNTIME_DIR/firered" "$MODEL_ROOT/fireredasr2-aed" "$MODEL_ROOT/fireredpunc"
  fi
  if ((INSTALL_QWEN3)); then
    rm -f "$BIN_DIR/voime-gpu-qwen3"
    rm -rf "$RUNTIME_DIR/qwen3" "$MODEL_ROOT/qwen3-asr-1.7b"
  fi
  echo "Selected GPU backends removed; final.backend is unchanged."
  exit 0
fi

if [[ ! -x "$BIN_DIR/voimed" && ! -f "$HOME/.local/bin/voimed" ]]; then
  echo "core install not found; install core before the GPU module" >&2
  exit 1
fi

require_commands() {
  local cmd missing=()
  for cmd in "$@"; do
    command -v "$cmd" >/dev/null 2>&1 || missing+=("$cmd")
  done
  if ((${#missing[@]})); then
    echo "missing required commands: ${missing[*]}" >&2
    return 1
  fi
}

require_arch_packages() {
  local package missing=()
  if ! command -v pacman >/dev/null 2>&1; then
    echo "non-Arch system: verify these dependencies manually: $*" >&2
    return 0
  fi
  for package in "$@"; do
    pacman -Qq "$package" >/dev/null 2>&1 || missing+=("$package")
  done
  if ((${#missing[@]} == 0)); then
    return 0
  fi
  echo "missing Arch Linux packages: ${missing[*]}" >&2
  if ((INSTALL_DEPS)); then
    echo "installing with pacman..."
    sudo pacman -S --needed --noconfirm "${missing[@]}"
  else
    echo "install them with: sudo pacman -S --needed ${missing[*]}" >&2
    echo "or rerun this script with --install-deps" >&2
    return 1
  fi
}

check_dependencies() {
  local commands=(cargo cc make cmake clang pkg-config)
  local packages=(gcc make pkgconf cmake clang)
  if ((INSTALL_FIRERED)); then
    case "$FIRERED_RUNTIME" in
      cuda) packages+=(cuda) ;;
      migraphx) packages+=(migraphx) ;;
    esac
  fi
  if ((INSTALL_QWEN3)); then
    case "$QWEN_RUNTIME" in
      vulkan)
        packages+=(vulkan-headers vulkan-icd-loader shaderc glslang)
        if [[ "$GPU_VENDOR" == amd ]]; then
          packages+=(vulkan-radeon)
        fi
        ;;
      cuda) packages+=(cuda) ;;
    esac
  fi
  if ((INSTALL_DEPS)); then
    command -v sudo >/dev/null 2>&1 || packages+=(sudo)
  fi
  if ! command -v cargo >/dev/null 2>&1; then
    if command -v pacman >/dev/null 2>&1 && pacman -Qq rustup >/dev/null 2>&1; then
      echo "cargo is missing from PATH although rustup is installed; source ~/.cargo/env" >&2
      exit 1
    fi
    packages+=(rust)
  fi
  command -v cc >/dev/null 2>&1 || packages+=(gcc)
  command -v make >/dev/null 2>&1 || packages+=(make)
  command -v cmake >/dev/null 2>&1 || packages+=(cmake)
  command -v clang >/dev/null 2>&1 || packages+=(clang)
  command -v pkg-config >/dev/null 2>&1 || packages+=(pkgconf)
  # Deduplicate while preserving order.
  local unique=() package seen
  for package in "${packages[@]}"; do
    seen=0
    for existing in "${unique[@]}"; do
      [[ "$existing" == "$package" ]] && { seen=1; break; }
    done
    ((seen)) || unique+=("$package")
  done
  require_arch_packages "${unique[@]}" || exit 1
  require_commands "${commands[@]}"
}

check_dependencies

build_firered() {
  if [[ -n "$MODEL_SOURCE" && -x "$MODEL_SOURCE/bin/voime-gpu-firered" ]]; then
    echo "Installing precompiled FireRed worker from $MODEL_SOURCE..."
    install -m 0755 "$MODEL_SOURCE/bin/voime-gpu-firered" "$BIN_DIR/voime-gpu-firered"
    return
  fi
  local features=()
  case "$FIRERED_RUNTIME" in
    cuda) features=(--features cuda) ;;
    migraphx) features=(--features migraphx) ;;
    directml) features=(--features directml) ;;
  esac
  echo "Building FireRed worker ($FIRERED_RUNTIME)..."
  (cd "$ROOT/crates/voime-gpu-firered" && cargo build --release "${features[@]}")
  install -m 0755 "$ROOT/crates/voime-gpu-firered/target/release/voime-gpu-firered" "$BIN_DIR/voime-gpu-firered"
}

build_qwen3() {
  if [[ -n "$MODEL_SOURCE" && -x "$MODEL_SOURCE/bin/voime-gpu-qwen3" ]]; then
    echo "Installing precompiled Qwen worker from $MODEL_SOURCE..."
    install -m 0755 "$MODEL_SOURCE/bin/voime-gpu-qwen3" "$BIN_DIR/voime-gpu-qwen3"
    return
  fi
  local features=()
  case "$QWEN_RUNTIME" in
    vulkan) features=(--features vulkan) ;;
    cuda) features=(--features cuda) ;;
  esac
  echo "Building Qwen worker ($QWEN_RUNTIME)..."
  (cd "$ROOT/crates/voime-gpu-qwen3" && cargo build --release "${features[@]}")
  install -m 0755 "$ROOT/crates/voime-gpu-qwen3/target/release/voime-gpu-qwen3" "$BIN_DIR/voime-gpu-qwen3"
}

install_models() {
  local model="$1"
  if [[ ! -f "$MODEL_SOURCE/$model/manifest.json" ]]; then
    echo "missing model manifest: $MODEL_SOURCE/$model/manifest.json" >&2
    exit 1
  fi
  mkdir -p "$MODEL_ROOT"
  rm -rf "$MODEL_ROOT/$model.tmp"
  cp -a "$MODEL_SOURCE/$model" "$MODEL_ROOT/$model.tmp"
  rm -rf "$MODEL_ROOT/$model"
  mv "$MODEL_ROOT/$model.tmp" "$MODEL_ROOT/$model"
}

if ((INSTALL_FIRERED)); then
  build_firered
  echo "FireRed runtime selected: $FIRERED_RUNTIME (worker loads only this EP)."
  if ((INSTALL_MODELS)); then
    if [[ -z "$MODEL_SOURCE" ]]; then
      [[ -d "$ROOT/models/fireredasr2-aed" ]] && MODEL_SOURCE="$ROOT/models" || {
        echo "FireRed models not found; pass --source-dir or --no-models" >&2; exit 1; }
    fi
    install_models fireredasr2-aed
    install_models fireredpunc
  fi
fi

if ((INSTALL_QWEN3)); then
  build_qwen3
  echo "Qwen runtime selected: $QWEN_RUNTIME (AMD Linux only supports vulkan)."
  if ((INSTALL_MODELS)); then
    if [[ -z "$MODEL_SOURCE" ]]; then
      [[ -d "$ROOT/models/qwen3-asr-1.7b" ]] && MODEL_SOURCE="$ROOT/models" || {
        echo "Qwen model not found; pass --source-dir or --no-models" >&2; exit 1; }
    fi
    install_models qwen3-asr-1.7b
  fi
fi

write_backend() {
  local config="$1" backend="$2" tmp
  mkdir -p "$(dirname "$config")"
  tmp="$(mktemp "${config}.XXXXXX")"
  if [[ -f "$config" ]]; then
    awk -v backend="$backend" '
      BEGIN { in_final = 0; written = 0; has_final = 0 }
      /^[[:space:]]*\[final\][[:space:]]*$/ {
        if (in_final && !written) { print "backend = \"" backend "\""; written = 1 }
        in_final = 1; has_final = 1; print; next
      }
      /^[[:space:]]*\[/ {
        if (in_final && !written) { print "backend = \"" backend "\""; written = 1 }
        in_final = 0; print; next
      }
      in_final && /^[[:space:]]*backend[[:space:]]*=/ {
        if (!written) { print "backend = \"" backend "\""; written = 1 }
        next
      }
      { print }
      END {
        if (in_final && !written) { print "backend = \"" backend "\""; written = 1 }
        if (!has_final) {
          print "[final]"
          print "backend = \"" backend "\""
        }
      }
    ' "$config" > "$tmp"
  else
    printf '[final]\nbackend = "%s"\n' "$backend" > "$tmp"
  fi
  mv -f "$tmp" "$config"
}

if ((EXPLICIT_BACKEND)); then
  CONFIG_DIR="${XDG_CONFIG_HOME:-$HOME/.config}/voime"
  CONFIG="$CONFIG_DIR/config.toml"
  write_backend "$CONFIG" "$BACKEND"
  echo "final.backend = $BACKEND (restart voimed to apply)"
fi

echo "GPU module installed. Run: $BIN_DIR/voime doctor"
