#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
PREFIX="/usr"
BUILD_DIR="$ROOT/build/fcitx"
BUILD=1

usage() {
  cat <<'EOF'
用法: packaging/linux/install_fcitx_addon.sh [选项]

构建并安装 Voime 的 Fcitx5 addon（voime.so 与 addon 描述文件）。
addon 不注册输入法条目，不需要加入输入法组。

选项:
  --prefix DIR     安装前缀，默认 /usr
  --build-dir DIR  CMake 构建目录，默认 <repo>/build/fcitx
  --no-build       使用已有构建产物，不重新编译
  -h, --help       显示帮助

示例:
  packaging/linux/install_fcitx_addon.sh
  packaging/linux/install_fcitx_addon.sh --prefix /usr/local
  packaging/linux/install_fcitx_addon.sh --no-build
EOF
}

while [ "$#" -gt 0 ]; do
  case "$1" in
    --prefix)
      [ "$#" -ge 2 ] || { echo "install_fcitx_addon: --prefix 需要目录" >&2; exit 2; }
      PREFIX="$2"
      shift 2
      ;;
    --build-dir)
      [ "$#" -ge 2 ] || { echo "install_fcitx_addon: --build-dir 需要目录" >&2; exit 2; }
      BUILD_DIR="$2"
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
      echo "install_fcitx_addon: 未知参数 $1" >&2
      usage >&2
      exit 2
      ;;
  esac
done

case "$PREFIX" in
  /*) ;;
  *)
    echo "install_fcitx_addon: --prefix 必须是绝对路径" >&2
    exit 2
    ;;
esac

can_write_prefix() {
  local dir="$1"
  while [ ! -e "$dir" ]; do
    local parent
    parent="$(dirname "$dir")"
    [ "$parent" = "$dir" ] && return 1
    dir="$parent"
  done
  [ -w "$dir" ]
}

if ! command -v cmake >/dev/null 2>&1; then
  echo "install_fcitx_addon: 找不到 cmake，请先安装 CMake" >&2
  exit 1
fi
if ! command -v pkg-config >/dev/null 2>&1; then
  echo "install_fcitx_addon: 找不到 pkg-config，请先安装 pkg-config" >&2
  exit 1
fi
if ! pkg-config --exists Fcitx5Core; then
  echo "install_fcitx_addon: 找不到 Fcitx5Core 开发头文件（Arch: fcitx5；Debian/Ubuntu: libfcitx5core-dev）" >&2
  exit 1
fi

echo "install_fcitx_addon: 配置 $BUILD_DIR"
cmake -S "$ROOT/frontends/fcitx" -B "$BUILD_DIR" \
  -DCMAKE_INSTALL_PREFIX="$PREFIX" \
  -DCMAKE_BUILD_TYPE=Release

if [ "$BUILD" -eq 1 ]; then
  echo "install_fcitx_addon: 构建 Fcitx5 addon"
  cmake --build "$BUILD_DIR" --target voime --parallel
fi

if [ ! -f "$BUILD_DIR/voime.so" ]; then
  echo "install_fcitx_addon: 找不到 $BUILD_DIR/voime.so，请先运行 cmake --build 或去掉 --no-build" >&2
  exit 1
fi

# 安装目录可能是 /usr 这类受保护路径；只对安装步骤提权，构建仍以当前用户执行。
if [ "$(id -u)" -ne 0 ] && ! can_write_prefix "$PREFIX"; then
  if ! command -v sudo >/dev/null 2>&1; then
    echo "install_fcitx_addon: 没有写入 $PREFIX 的权限，请用 root 运行或安装 sudo" >&2
    exit 1
  fi
  echo "install_fcitx_addon: 安装到 $PREFIX（需要 sudo）"
  sudo cmake --install "$BUILD_DIR"
else
  echo "install_fcitx_addon: 安装到 $PREFIX"
  cmake --install "$BUILD_DIR"
fi

echo "install_fcitx_addon: 已安装 Fcitx5 addon"

cat <<EOF

下一步:
  fcitx5 -r          # 重启 Fcitx5 加载 addon
  fcitx5-configtool # 在 Addons 页面确认 Voime 已加载

修改 Mode / Backend / Hotkey:
  ~/.config/fcitx5/conf/voime.conf
EOF
