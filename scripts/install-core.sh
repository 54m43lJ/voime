#!/usr/bin/env bash
# Voime core install (Linux): daemon, CLI, Fcitx addon, core models.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PREFIX="${VOIME_PREFIX:-$HOME/.local}"
MODEL_ROOT="${VOIME_MODEL_ROOT:-${XDG_DATA_HOME:-$HOME/.local/share}/voime/models}"
MODEL_SOURCE=""
INSTALL_ADDON=1
INSTALL_MODELS=1
INSTALL_SERVICE=1
INSTALL_DEPS=0
UNINSTALL=0

usage() {
  cat <<'EOF'
Usage: scripts/install-core.sh [options]

Options:
  --no-addon            Skip the system Fcitx5 addon installation.
  --no-models           Skip copying core models.
  --no-service          Do not install the systemd user service.
  --model-dir <dir>     Model root (default: $XDG_DATA_HOME/voime/models).
  --source-dir <dir>    Offline source containing firered-vad/ (model.onnx + cmvn.ark) and sensevoice-small/.
  --prefix <dir>        Install prefix (default: $HOME/.local).
  --install-deps        Install missing Arch Linux packages with pacman.
  --uninstall           Remove the core installation.
  -h, --help            Show this help.
EOF
}

while (($#)); do
  case "$1" in
    --no-addon) INSTALL_ADDON=0 ;;
    --no-models) INSTALL_MODELS=0 ;;
    --no-service) INSTALL_SERVICE=0 ;;
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
  local commands=(cargo cc make pkg-config python3)
  local packages=(gcc make pkgconf alsa-lib openssl python)
  if ((INSTALL_ADDON)); then
    commands+=(cmake sudo)
    packages+=(cmake fcitx5)
  fi
  if ((INSTALL_SERVICE)); then
    packages+=(systemd)
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
  command -v pkg-config >/dev/null 2>&1 || packages+=(pkgconf)
  if ((INSTALL_ADDON)); then
    command -v cmake >/dev/null 2>&1 || packages+=(cmake)
    command -v sudo >/dev/null 2>&1 || packages+=(sudo)
  fi
  require_arch_packages "${packages[@]}" || exit 1
  require_commands "${commands[@]}"
}

remove_addon() {
  local libdir
  libdir="$(pkg-config --variable=libdir Fcitx5Core 2>/dev/null || echo /usr/lib)"
  sudo rm -f "$libdir/fcitx5/voime.so" /usr/share/fcitx5/addon/voime.conf
  sudo rm -f /usr/share/fcitx5/inputmethod/voime.conf
}

if ((UNINSTALL)); then
  rm -f "$BIN_DIR/voimed" "$BIN_DIR/voime"
  if ((INSTALL_ADDON)); then remove_addon; fi
  if ((INSTALL_SERVICE)); then
    systemctl --user disable --now voime.service >/dev/null 2>&1 || true
    rm -f "$HOME/.config/systemd/user/voime.service"
    systemctl --user daemon-reload >/dev/null 2>&1 || true
  fi
  echo "Voime core removed (models left in $MODEL_ROOT)."
  exit 0
fi

check_dependencies

echo "Building voimed and voime..."
(cd "$ROOT" && cargo build --release -p voimed -p voime)
mkdir -p "$BIN_DIR"
install -m 0755 "$ROOT/target/release/voimed" "$BIN_DIR/voimed"
install -m 0755 "$ROOT/target/release/voime" "$BIN_DIR/voime"

if ((INSTALL_MODELS)); then
  model_args=(--model-root "$MODEL_ROOT")
  if [[ -n "$MODEL_SOURCE" ]]; then
    model_args+=(--source-dir "$MODEL_SOURCE" --no-download)
  fi
  python3 "$ROOT/scripts/install-core-models.py" "${model_args[@]}"
fi

if ((INSTALL_ADDON)); then
  if ! command -v cmake >/dev/null 2>&1 || ! pkg-config --exists Fcitx5Core; then
    echo "Fcitx5 development files and cmake are required for the addon" >&2
    echo "re-run with --no-addon to skip it" >&2
    exit 1
  fi
  if ! pkg-config --atleast-version=5.1.23 Fcitx5Core; then
    echo "Fcitx5 >= 5.1.23 is required for the addon" >&2
    echo "re-run with --no-addon to skip it" >&2
    exit 1
  fi
  BUILD_DIR="$(mktemp -d)"
  trap 'rm -rf "$BUILD_DIR"' EXIT
  cmake -S "$ROOT/frontends/fcitx" -B "$BUILD_DIR" -DCMAKE_BUILD_TYPE=Release >/dev/null
  cmake --build "$BUILD_DIR" -j"$(nproc)" >/dev/null
  LIBDIR="$(pkg-config --variable=libdir Fcitx5Core)"
  if ! sudo install -d "$LIBDIR/fcitx5" /usr/share/fcitx5/addon ||
     ! sudo install -m 0755 "$BUILD_DIR/voime.so" "$LIBDIR/fcitx5/voime.so" ||
     ! sudo install -m 0644 "$BUILD_DIR/voime.conf" /usr/share/fcitx5/addon/voime.conf; then
    echo "Fcitx5 addon installation failed (sudo refused or path not writable)." >&2
    echo "re-run with --no-addon to skip the system addon." >&2
    exit 1
  fi
  sudo rm -f /usr/share/fcitx5/inputmethod/voime.conf
  echo "Fcitx5 addon installed. Restart Fcitx5 to load it."
fi

if ((INSTALL_SERVICE)); then
  SERVICE_DIR="$HOME/.config/systemd/user"
  mkdir -p "$SERVICE_DIR"
  sed "s|@BINDIR@|$BIN_DIR|g" "$ROOT/packaging/linux/voime.service" > "$SERVICE_DIR/voime.service"
  systemctl --user daemon-reload >/dev/null 2>&1 || true
  systemctl --user enable --now voime.service >/dev/null 2>&1 || \
    echo "service installed; enable it with: systemctl --user enable --now voime.service"
fi

echo "Voime core installed. Run: $BIN_DIR/voime doctor"
