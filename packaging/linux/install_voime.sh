#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
PREFIX="${HOME}/.local"
BINDIR=""
BUILD=1

usage() {
  cat <<'EOF'
用法: packaging/linux/install_voime.sh [选项]

安装 voime（TUI / ctl / doctor）到 $PREFIX/bin。

选项:
  --prefix DIR     安装前缀，默认 ~/.local
  --bindir DIR     仅指定二进制目录，默认 $PREFIX/bin
  --no-build       使用已有的 target/release/voime，不重新构建
  -h, --help       显示帮助

示例:
  packaging/linux/install_voime.sh
  packaging/linux/install_voime.sh --prefix /usr/local
  packaging/linux/install_voime.sh --no-build
EOF
}

while [ "$#" -gt 0 ]; do
  case "$1" in
    --prefix)
      [ "$#" -ge 2 ] || { echo "install_voime: --prefix 需要目录" >&2; exit 2; }
      PREFIX="$2"
      shift 2
      ;;
    --bindir)
      [ "$#" -ge 2 ] || { echo "install_voime: --bindir 需要目录" >&2; exit 2; }
      BINDIR="$2"
      shift 2
      ;;
    --no-build)
      BUILD=0
      shift
      ;;
    -h | --help)
      usage
      exit 0
      ;;
    *)
      echo "install_voime: 未知参数 $1" >&2
      usage >&2
      exit 2
      ;;
  esac
done

BINDIR="${BINDIR:-$PREFIX/bin}"

if [ "$BUILD" -eq 1 ]; then
  echo "install_voime: 构建 voime"
  cargo build --release -p voime --manifest-path "$ROOT/Cargo.toml"
fi

if [ ! -x "$ROOT/target/release/voime" ]; then
  echo "install_voime: 找不到 $ROOT/target/release/voime，请先运行 cargo build --release" >&2
  exit 1
fi

install -d "$BINDIR"
install -m 755 "$ROOT/target/release/voime" "$BINDIR/voime"
echo "install_voime: 已安装 $BINDIR/voime"

cat <<EOF

下一步:
  $BINDIR/voime
  $BINDIR/voime ctl status
  $BINDIR/voime doctor

如果 $BINDIR 不在 PATH 中，请将其加入 shell 的 PATH。
EOF
