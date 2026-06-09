#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
KERNEL_SRC="${KERNEL_SRC:-$ROOT/reference/Linux-Kernel_MiSTer}"
KERNEL_BUILD="${KERNEL_BUILD:-$ROOT/build/fbwc-kernel}"
MODULE_DIR="$ROOT/kernel/fbwc"
export CCACHE_DISABLE="${CCACHE_DISABLE:-1}"
CROSS_COMPILE="${CROSS_COMPILE:-arm-none-linux-gnueabihf-}"
KERNEL_LOCALVERSION="${KERNEL_LOCALVERSION:--MiSTer}"
IMAGE="${MISTER_FBWC_DOCKER_IMAGE:-mister-magik-fbwc-builder:arm-gcc-10.2}"

build_in_docker() {
  if [[ "${MISTER_FBWC_IN_DOCKER:-0}" == "1" ]]; then
    echo "missing cross compiler in Docker: ${CROSS_COMPILE}gcc" >&2
    exit 1
  fi
  if ! command -v docker >/dev/null 2>&1; then
    echo "missing cross compiler: ${CROSS_COMPILE}gcc" >&2
    echo "docker is also unavailable; install the MiSTer ARM toolchain or Docker" >&2
    exit 1
  fi
  if ! docker info >/dev/null 2>&1; then
    echo "docker is installed but the daemon is not reachable" >&2
    docker info >/dev/null
  fi

  echo "cross compiler not found; building fbwc module in Docker image $IMAGE"
  docker build \
    --platform linux/amd64 \
    -t "$IMAGE" \
    -f "$ROOT/kernel/fbwc/Dockerfile" \
    "$ROOT/kernel/fbwc"
  docker run --rm \
    --platform linux/amd64 \
    -v "$ROOT:/src" \
    -w /src \
    -e MISTER_FBWC_IN_DOCKER=1 \
    -e CCACHE_DISABLE=1 \
    "$IMAGE" \
    bash -lc 'GCC_BIN="$(echo /usr/local/bin/gcc-arm-*/bin)"; export PATH="$GCC_BIN:$PATH"; scripts/build-fbwc-module.sh'
  exit $?
}

if [[ ! -d "$KERNEL_SRC" ]]; then
  echo "missing kernel source: $KERNEL_SRC" >&2
  echo "clone MiSTer-devel/Linux-Kernel_MiSTer branch MiSTer-v5.15 into reference/Linux-Kernel_MiSTer" >&2
  exit 1
fi

if ! command -v "${CROSS_COMPILE}gcc" >/dev/null 2>&1; then
  build_in_docker
fi

mkdir -p "$KERNEL_BUILD"

if [[ ! -f "$KERNEL_BUILD/.config" ]]; then
  echo "preparing MiSTer kernel build tree"
  make -C "$KERNEL_SRC" O="$KERNEL_BUILD" ARCH=arm CROSS_COMPILE="$CROSS_COMPILE" LOCALVERSION="$KERNEL_LOCALVERSION" MiSTer_defconfig
fi
make -C "$KERNEL_SRC" O="$KERNEL_BUILD" ARCH=arm CROSS_COMPILE="$CROSS_COMPILE" LOCALVERSION="$KERNEL_LOCALVERSION" modules_prepare

make -C "$MODULE_DIR" KERNEL_SRC="$KERNEL_SRC" KERNEL_BUILD="$KERNEL_BUILD" ARCH=arm CROSS_COMPILE="$CROSS_COMPILE" LOCALVERSION="$KERNEL_LOCALVERSION" all

echo "$MODULE_DIR/mister_magik_fbwc.ko"
