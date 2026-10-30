#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
PREFIX="${HOME}/.local"
BINDIR=""
BUILD=1
SYSTEMD=1

usage() {
  cat <<'EOF'
用法: packaging/linux/install_voimed.sh [选项]

安装 voimed 到 $PREFIX/bin，并安装 systemd user unit。

选项:
  --prefix DIR     安装前缀，默认 ~/.local
  --bindir DIR     仅指定二进制目录，默认 $PREFIX/bin
  --no-build       使用已有的 target/release/voimed，不重新构建
  --no-systemd     不安装 systemd user unit
  -h, --help       显示帮助

示例:
  packaging/linux/install_voimed.sh
  packaging/linux/install_voimed.sh --prefix /usr/local
  packaging/linux/install_voimed.sh --no-build
EOF
}

while [ "$#" -gt 0 ]; do
  case "$1" in
    --prefix)
      [ "$#" -ge 2 ] || { echo "install_voimed: --prefix 需要目录" >&2; exit 2; }
      PREFIX="$2"
      shift 2
      ;;
    --bindir)
      [ "$#" -ge 2 ] || { echo "install_voimed: --bindir 需要目录" >&2; exit 2; }
      BINDIR="$2"
      shift 2
      ;;
    --no-build)
      BUILD=0
      shift
      ;;
    --no-systemd)
      SYSTEMD=0
      shift
      ;;
    -h | --help)
      usage
      exit 0
      ;;
    *)
      echo "install_voimed: 未知参数 $1" >&2
      usage >&2
      exit 2
      ;;
  esac
done

BINDIR="${BINDIR:-$PREFIX/bin}"
SYSTEMD_DIR="${XDG_CONFIG_HOME:-$HOME/.config}/systemd/user"

if [ "$BUILD" -eq 1 ]; then
  echo "install_voimed: 构建 voimed"
  cargo build --release -p voimed --manifest-path "$ROOT/Cargo.toml"
fi

if [ ! -x "$ROOT/target/release/voimed" ]; then
  echo "install_voimed: 找不到 $ROOT/target/release/voimed，请先运行 cargo build --release" >&2
  exit 1
fi

install -d "$BINDIR"
install -m 755 "$ROOT/target/release/voimed" "$BINDIR/voimed"
echo "install_voimed: 已安装 $BINDIR/voimed"

if [ "$SYSTEMD" -eq 1 ]; then
  install -d "$SYSTEMD_DIR"
  awk -v exec="ExecStart=\"$BINDIR/voimed\"" \
    '/^ExecStart=/{print exec; next} {print}' \
    "$ROOT/packaging/linux/voime.service" >"$SYSTEMD_DIR/voime.service"
  echo "install_voimed: 已安装 $SYSTEMD_DIR/voime.service"
  if command -v systemctl >/dev/null 2>&1; then
    systemctl --user daemon-reload >/dev/null 2>&1 || true
  fi
  cat <<EOF

下一步:
  systemctl --user enable --now voime.service
  systemctl --user status voime.service

也可以手动启动:
  $BINDIR/voimed
EOF
else
  echo "install_voimed: 跳过 systemd unit（--no-systemd）"
fi
