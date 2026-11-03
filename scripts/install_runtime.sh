#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PREFIX="${HOME}/.local"
BINDIR=""
RUNTIME=""
PROVIDER_DIR=""
BUILD=1
SYSTEMD=1
FEATURE=""
PROVIDER_LIB=""

usage() {
  cat <<'EOF'
用法: scripts/install_runtime.sh --runtime <name> [选项]

从源码构建自包含的 voime-gpu：静态链接 ONNX Runtime，并把对应 GPU provider
动态库放在 worker 同目录。CPU daemon 与 GPU worker 使用各自独立的内置 ORT。

runtimes:
  linux-migraphx  Arch: migraphx（ROCm runtime）
  linux-cuda      Arch: cuda cudnn

选项:
  --runtime NAME       必选，选择 GPU 后端
  --prefix DIR         安装前缀，默认 ~/.local
  --bindir DIR         仅指定命令目录，默认 $PREFIX/bin
  --provider-dir DIR   从该目录复制 ORT provider 动态库，默认 target/release
  --no-build           使用已有 target/release/voime-gpu，不重新构建
  --no-systemd         不写 systemd user drop-in
  -h, --help           显示帮助

示例:
  scripts/install_runtime.sh --runtime linux-migraphx
  scripts/install_runtime.sh --runtime linux-cuda --prefix /usr/local
  scripts/install_runtime.sh --runtime linux-migraphx --no-build
EOF
}

while [ "$#" -gt 0 ]; do
  case "$1" in
    --runtime)
      [ "$#" -ge 2 ] || { echo "install_runtime: --runtime 需要名称" >&2; exit 2; }
      RUNTIME="$2"
      shift 2
      ;;
    --prefix)
      [ "$#" -ge 2 ] || { echo "install_runtime: --prefix 需要目录" >&2; exit 2; }
      PREFIX="$2"
      shift 2
      ;;
    --bindir)
      [ "$#" -ge 2 ] || { echo "install_runtime: --bindir 需要目录" >&2; exit 2; }
      BINDIR="$2"
      shift 2
      ;;
    --provider-dir)
      [ "$#" -ge 2 ] || { echo "install_runtime: --provider-dir 需要目录" >&2; exit 2; }
      PROVIDER_DIR="$2"
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
      echo "install_runtime: 未知参数 $1" >&2
      usage >&2
      exit 2
      ;;
  esac
done

case "$RUNTIME" in
  linux-migraphx)
    FEATURE="migraphx"
    PROVIDER_LIB="libonnxruntime_providers_migraphx.so"
    ;;
  linux-cuda)
    FEATURE="cuda"
    PROVIDER_LIB="libonnxruntime_providers_cuda.so"
    ;;
  "")
    echo "install_runtime: 需要 --runtime" >&2
    usage >&2
    exit 2
    ;;
  windows-directml | windows-cuda)
    echo "install_runtime: Windows 请使用 scripts/install_runtime.ps1" >&2
    exit 2
    ;;
  *)
    echo "install_runtime: 未知 runtime $RUNTIME" >&2
    usage >&2
    exit 2
    ;;
esac

case "$PREFIX" in
  /*) ;;
  *)
    echo "install_runtime: --prefix 必须是绝对路径" >&2
    exit 2
    ;;
esac

BINDIR="${BINDIR:-$PREFIX/bin}"
case "$BINDIR" in
  /*) ;;
  *)
    echo "install_runtime: --bindir 必须是绝对路径" >&2
    exit 2
    ;;
esac

if [ "$BUILD" -eq 1 ]; then
  if ! command -v cargo >/dev/null 2>&1; then
    echo "install_runtime: 找不到 cargo，请先安装 Rust 工具链" >&2
    exit 1
  fi
  echo "install_runtime: 构建 voime-gpu（$RUNTIME，feature=$FEATURE）"
  cargo build --release -p voime-gpu --features "$FEATURE" --manifest-path "$ROOT/Cargo.toml"
fi

BINARY="$ROOT/target/release/voime-gpu"
if [ ! -x "$BINARY" ]; then
  echo "install_runtime: 找不到 $BINARY，请先运行 cargo build 或去掉 --no-build" >&2
  exit 1
fi

SOURCE_DIR="${PROVIDER_DIR:-$ROOT/target/release}"
if [ ! -d "$SOURCE_DIR" ]; then
  echo "install_runtime: provider 目录不存在: $SOURCE_DIR" >&2
  exit 1
fi

BUNDLE_DIR="$PREFIX/lib/voime-gpu/$RUNTIME"
install -d "$BUNDLE_DIR" "$BINDIR"
install -m 755 "$BINARY" "$BUNDLE_DIR/voime-gpu"
rm -f "$BUNDLE_DIR"/libonnxruntime*.so*

copied=0
while IFS= read -r -d '' file; do
  cp -L "$file" "$BUNDLE_DIR/"
  copied=$((copied + 1))
done < <(find "$SOURCE_DIR" -maxdepth 1 \( -type f -o -type l \) \
  -name 'libonnxruntime*.so*' -print0)

if [ ! -f "$BUNDLE_DIR/$PROVIDER_LIB" ] \
  || [ ! -f "$BUNDLE_DIR/libonnxruntime_providers_shared.so" ]; then
  echo "install_runtime: 缺少 $PROVIDER_LIB 或 libonnxruntime_providers_shared.so；请用 --features $FEATURE 构建，或用 --provider-dir 指定 provider 目录" >&2
  exit 1
fi

ln -sfn "$BUNDLE_DIR/voime-gpu" "$BINDIR/voime-gpu"

cat >"$BUNDLE_DIR/runtime.txt" <<EOF
runtime=$RUNTIME
feature=$FEATURE
worker=$BUNDLE_DIR/voime-gpu
provider=$PROVIDER_LIB
built_at=$(date -u +%Y-%m-%dT%H:%M:%SZ)
EOF

echo "install_runtime: 已安装 $BUNDLE_DIR/voime-gpu（$copied 个 ONNX Runtime / provider 动态库）"
echo "install_runtime: 命令入口 $BINDIR/voime-gpu"

if [ "$SYSTEMD" -eq 1 ]; then
  SYSTEMD_DIR="${XDG_CONFIG_HOME:-$HOME/.config}/systemd/user/voime.service.d"
  install -d "$SYSTEMD_DIR"
  cat >"$SYSTEMD_DIR/runtime.conf" <<EOF
[Service]
Environment="VOIME_GPU_BIN=$BUNDLE_DIR/voime-gpu"
Environment="VOIME_GPU_RUNTIME_DIR=$BUNDLE_DIR"
EOF
  echo "install_runtime: 已写入 $SYSTEMD_DIR/runtime.conf"
  if command -v systemctl >/dev/null 2>&1; then
    systemctl --user daemon-reload >/dev/null 2>&1 || true
  fi
fi

cat <<EOF

下一步:
  systemctl --user restart voime.service
  voime ctl backend gpu
  voime doctor

手动启动时设置:
  export VOIME_GPU_BIN=$BUNDLE_DIR/voime-gpu
  export VOIME_GPU_RUNTIME_DIR=$BUNDLE_DIR
EOF
