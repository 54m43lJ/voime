#!/usr/bin/env bash
set -euo pipefail

# Install an opt-in voime-gpu runtime package. Runtime packages are distributed
# separately from the base install and are never placed inside a model package.

usage() {
  cat >&2 <<'EOF'
usage: install_runtime.sh --runtime <name> (--archive <file> | --url <url>)

runtimes:
  linux-migraphx  windows-directml  linux-cuda  windows-cuda
EOF
  exit 2
}

runtime=""
archive=""
url=""
while [ $# -gt 0 ]; do
  case "$1" in
    --runtime) runtime="${2:-}"; shift 2 ;;
    --archive) archive="${2:-}"; shift 2 ;;
    --url) url="${2:-}"; shift 2 ;;
    -h|--help) usage ;;
    *) echo "install_runtime: unknown argument: $1" >&2; usage ;;
  esac
done

case "$runtime" in
  linux-migraphx|windows-directml|linux-cuda|windows-cuda) ;;
  *) echo "install_runtime: 需要 --runtime" >&2; usage ;;
esac
if [ -z "$archive" ] && [ -z "$url" ]; then
  echo "install_runtime: 需要 --archive 或 --url；runtime 包需显式 opt-in" >&2
  exit 2
fi

base="${VOIME_RUNTIME_DIR:-${XDG_DATA_HOME:-$HOME/.local/share}/voime/runtime}"
target="$base/$runtime"
mkdir -p "$target"

temporary="$(mktemp -d "${TMPDIR:-/tmp}/voime-runtime.XXXXXX")"
trap 'rm -rf "$temporary"' EXIT

if [ -n "$archive" ]; then
  package="$archive"
else
  package="$temporary/runtime.tar.gz"
  echo "install_runtime: 下载 $url"
  if command -v curl >/dev/null 2>&1; then
    curl --fail --location --output "$package" "$url"
  else
    wget --output-document="$package" "$url"
  fi
fi

tar -xf "$package" -C "$temporary"
if [ -d "$temporary/$runtime" ]; then
  source_dir="$temporary/$runtime"
elif [ -d "$temporary/runtime" ]; then
  source_dir="$temporary/runtime"
else
  source_dir="$temporary"
fi

cp -a "$source_dir"/. "$target"/
echo "install_runtime: 已安装 $runtime -> $target"
echo "install_runtime: 设置 VOIME_RUNTIME_DIR=$base 或重启后端后再选择 GPU Final"
