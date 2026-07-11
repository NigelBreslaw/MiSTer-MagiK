#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
KERNEL_SRC="${KERNEL_SRC:-/private/tmp/Linux-Kernel_MiSTer}"
KERNEL_BUILD="${KERNEL_BUILD:-$ROOT/build/scanout-slots-kernel}"
MODULE_DIR="$ROOT/kernel/scanout-slots"
OUT_DIR="$ROOT/build/scanout-slots"
CROSS_COMPILE="${CROSS_COMPILE:-arm-linux-gnueabihf-}"
LOCALVERSION="${LOCALVERSION:--MiSTer}"
IMAGE="${MISTER_ARM_BUILD_IMAGE:-mister-magik-cross-armv7:ubuntu20-arm64}"

usage() {
  cat <<'EOF'
Usage:
  scripts/build-scanout-slots-module.sh

Builds the production mister_magik_scanout_slots.ko against the MiSTer 5.15
kernel source. Set KERNEL_SRC to override the source checkout.
EOF
}

case "${1:-}" in
  -h|--help)
    usage
    exit 0
    ;;
  "")
    ;;
  *)
    echo "unknown argument: $1" >&2
    usage >&2
    exit 2
    ;;
esac

if [[ ! -d "$KERNEL_SRC" ]]; then
  echo "missing kernel source: $KERNEL_SRC" >&2
  echo "clone MiSTer-devel/Linux-Kernel_MiSTer branch MiSTer-v5.15 or set KERNEL_SRC" >&2
  exit 1
fi

mkdir -p "$KERNEL_BUILD" "$OUT_DIR"

build_commands=$(cat <<'EOS'
set -euo pipefail
if [[ ! -f "$KERNEL_BUILD/.config" ]]; then
  make -C "$KERNEL_SRC" O="$KERNEL_BUILD" ARCH=arm CROSS_COMPILE="$CROSS_COMPILE" LOCALVERSION="$LOCALVERSION" MiSTer_defconfig
fi
make -C "$KERNEL_SRC" O="$KERNEL_BUILD" ARCH=arm CROSS_COMPILE="$CROSS_COMPILE" LOCALVERSION="$LOCALVERSION" modules_prepare
make -C "$MODULE_DIR" KERNEL_SRC="$KERNEL_SRC" KERNEL_BUILD="$KERNEL_BUILD" ARCH=arm CROSS_COMPILE="$CROSS_COMPILE" LOCALVERSION="$LOCALVERSION" all
cp "$MODULE_DIR/mister_magik_scanout_slots.ko" "$OUT_DIR/mister_magik_scanout_slots.ko"
if command -v "${CROSS_COMPILE}modinfo" >/dev/null 2>&1; then
  "${CROSS_COMPILE}modinfo" "$OUT_DIR/mister_magik_scanout_slots.ko" > "$OUT_DIR/modinfo.txt" || true
elif command -v modinfo >/dev/null 2>&1; then
  modinfo "$OUT_DIR/mister_magik_scanout_slots.ko" > "$OUT_DIR/modinfo.txt" || true
else
  : > "$OUT_DIR/modinfo.txt"
fi
(cd "$OUT_DIR" && sha256sum mister_magik_scanout_slots.ko > SHA256SUMS)
echo "$OUT_DIR/mister_magik_scanout_slots.ko"
EOS
)

if command -v "${CROSS_COMPILE}gcc" >/dev/null 2>&1; then
  KERNEL_SRC="$KERNEL_SRC" KERNEL_BUILD="$KERNEL_BUILD" MODULE_DIR="$MODULE_DIR" OUT_DIR="$OUT_DIR" \
    CROSS_COMPILE="$CROSS_COMPILE" LOCALVERSION="$LOCALVERSION" bash -lc "$build_commands"
  exit $?
fi

if ! command -v container >/dev/null 2>&1; then
  echo "missing cross compiler ${CROSS_COMPILE}gcc and Apple container runtime" >&2
  exit 1
fi

container run --arch arm64 --rm --cpus 8 --memory 8g \
  --volume "$ROOT:/project" \
  --volume "$KERNEL_SRC:$KERNEL_SRC" \
  --workdir /project \
  "$IMAGE" bash -lc "
    set -euo pipefail
    if command -v apt-get >/dev/null 2>&1; then
      apt-get update >/dev/null
      DEBIAN_FRONTEND=noninteractive apt-get install -y flex bison bc libssl-dev libelf-dev kmod >/dev/null
    fi
    export KERNEL_SRC='$KERNEL_SRC'
    export KERNEL_BUILD='/project/build/scanout-slots-kernel'
    export MODULE_DIR='/project/kernel/scanout-slots'
    export OUT_DIR='/project/build/scanout-slots'
    export CROSS_COMPILE='$CROSS_COMPILE'
    export LOCALVERSION='$LOCALVERSION'
    $build_commands
  "
