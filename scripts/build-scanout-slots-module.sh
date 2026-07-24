#!/usr/bin/env bash
# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
KERNEL_SRC="${KERNEL_SRC:-$ROOT/../Linux-Kernel_MiSTer}"
KERNEL_BUILD="${KERNEL_BUILD:-$ROOT/build/scanout-slots-kernel}"
MODULE_DIR="$ROOT/mister/platform/kernel/scanout-slots"
OUT_DIR="$ROOT/build/scanout-slots"
CROSS_COMPILE="${CROSS_COMPILE:-arm-linux-gnueabihf-}"
LOCALVERSION="${LOCALVERSION:--MiSTer}"
IMAGE="${MISTER_ARM_BUILD_IMAGE:-mister-magik-cross-armv7:ubuntu20-arm64}"
IMAGE_DOCKERFILE="$ROOT/apps/mister/Dockerfile.cross-armv7"
PINNED_KERNEL_REVISION="f0fb626acadd07f0718934826b143b6e4c9ce81c"
PINNED_FB_DRIVER_SHA256="b85ccabd33c3360c60873eb29deb933500b117759c3a3e898637a3e46e25312c"
PINNED_DT_SHA256="36d7f660df55253a9ba11ebce615f304b91c3d7c99be94173af443574ad28a95"
MODULE_INPUTS=(
  mister/platform/kernel/scanout-slots
  scripts/build-scanout-slots-module.sh
)
OBSERVED_SOURCE_REVISION="$(git -c safe.directory="$ROOT" -C "$ROOT" log -1 --format=%H -- "${MODULE_INPUTS[@]}")"
if [[ -z "$(git -c safe.directory="$ROOT" -C "$ROOT" status --porcelain --untracked-files=all -- "${MODULE_INPUTS[@]}")" ]]; then
  OBSERVED_SOURCE_DIRTY=0
else
  OBSERVED_SOURCE_DIRTY=1
fi
if [[ -z "${SOURCE_REVISION:-}" ]]; then
  SOURCE_REVISION="$OBSERVED_SOURCE_REVISION"
fi
if [[ -z "${SOURCE_DIRTY:-}" ]]; then
  SOURCE_DIRTY="$OBSERVED_SOURCE_DIRTY"
fi
[[ "$SOURCE_REVISION" =~ ^[0-9a-f]{40}$ ]]
[[ "$SOURCE_DIRTY" == 0 || "$SOURCE_DIRTY" == 1 ]]
if [[ "$SOURCE_DIRTY" == 0 && "$OBSERVED_SOURCE_DIRTY" != 0 ]]; then
  echo "SOURCE_DIRTY=0 cannot override a dirty checkout" >&2
  exit 1
fi
if [[ "$SOURCE_DIRTY" == 0 && "$SOURCE_REVISION" != "$OBSERVED_SOURCE_REVISION" ]]; then
  echo "a clean attestation must name the observed checkout revision" >&2
  exit 1
fi

usage() {
  cat <<'EOF'
Usage:
  scripts/build-scanout-slots-module.sh

Builds the production mister_magik_scanout_slots.ko against the MiSTer 5.15
kernel source. The default checkout is ../Linux-Kernel_MiSTer. Set KERNEL_SRC
to override it.
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
KERNEL_REVISION="$(git -c safe.directory="$KERNEL_SRC" -C "$KERNEL_SRC" rev-parse --verify 'HEAD^{commit}')"
if [[ ! "$KERNEL_REVISION" =~ ^[0-9a-f]{40}$ ]]; then
  echo "kernel source must be a git checkout at an exact revision" >&2
  exit 1
fi
if [[ "$KERNEL_REVISION" != "$PINNED_KERNEL_REVISION" ]]; then
  echo "kernel source revision $KERNEL_REVISION is not pinned $PINNED_KERNEL_REVISION" >&2
  exit 1
fi
if [[ "$(sha256sum "$KERNEL_SRC/drivers/video/fbdev/MiSTer_fb.c" | awk '{print $1}')" != "$PINNED_FB_DRIVER_SHA256" ]] ||
   [[ "$(sha256sum "$KERNEL_SRC/arch/arm/boot/dts/socfpga_cyclone5_de10_nano.dts" | awk '{print $1}')" != "$PINNED_DT_SHA256" ]]; then
  echo "pinned framebuffer driver or device-tree source identity mismatch" >&2
  exit 1
fi

mkdir -p "$KERNEL_BUILD" "$OUT_DIR"
RECEIPT="$OUT_DIR/build-receipt.txt"
if command -v "${CROSS_COMPILE}gcc" >/dev/null 2>&1; then
  COMPILER_IDENTITY="$("${CROSS_COMPILE}gcc" --version | sed -n '1p')"
  IMAGE_DIGEST="local-toolchain"
else
  if ! command -v container >/dev/null 2>&1; then
    echo "missing cross compiler ${CROSS_COMPILE}gcc and Apple container runtime" >&2
    exit 1
  fi
  IMAGE_DIGEST="$(container image inspect "$IMAGE" | sed -n 's/.*"digest" : "\(sha256:[0-9a-f]*\)".*/\1/p' | sed -n '1p')"
  if [[ ! "$IMAGE_DIGEST" =~ ^sha256:[0-9a-f]{64}$ ]]; then
    echo "cannot resolve immutable OCI digest for $IMAGE" >&2
    exit 1
  fi
  COMPILER_IDENTITY="$(sed -n 's/^compiler_identity=//p' "$RECEIPT" 2>/dev/null || true)"
  PROVENANCE_COMPILER="$(sed -n 's/^compiler=//p' "$OUT_DIR/provenance.txt" 2>/dev/null || true)"
  if [[ -z "$COMPILER_IDENTITY" || "$COMPILER_IDENTITY" != "$PROVENANCE_COMPILER" ]]; then
    COMPILER_IDENTITY="unverified"
  fi
fi
DOCKERFILE_SHA256="$(sha256sum "$IMAGE_DOCKERFILE" | awk '{print $1}')"
compute_build_identity() {
  local kernel_config_sha256="missing"
  if [[ -f "$KERNEL_BUILD/.config" ]]; then
    kernel_config_sha256="$(sha256sum "$KERNEL_BUILD/.config" | awk '{print $1}')"
  fi
  {
    printf '%s\n' \
      "receipt_version=2" \
      "$KERNEL_REVISION" \
      "$PINNED_FB_DRIVER_SHA256" \
      "$PINNED_DT_SHA256" \
      "$CROSS_COMPILE" \
      "$LOCALVERSION" \
      "$IMAGE" \
      "$IMAGE_DIGEST" \
      "$DOCKERFILE_SHA256" \
      "$COMPILER_IDENTITY" \
      "$kernel_config_sha256"
    cd "$ROOT"
    sha256sum \
      mister/platform/kernel/scanout-slots/mister_magik_scanout_slots.c \
      mister/platform/kernel/scanout-slots/mister_magik_scanout_slots_uapi.h \
      mister/platform/kernel/scanout-slots/mister_magik_scanout_platform.h \
      mister/platform/kernel/scanout-slots/mister_magik_scanout_policy.h \
      mister/platform/kernel/scanout-slots/Makefile \
      scripts/build-scanout-slots-module.sh
  } | sha256sum | awk '{print $1}'
}
BUILD_IDENTITY="$(compute_build_identity)"
if [[ -f "$RECEIPT" ]] &&
   grep -qx "receipt_version=2" "$RECEIPT" &&
   grep -qx "build_identity=$BUILD_IDENTITY" "$RECEIPT" &&
   grep -Fqx "compiler_identity=$COMPILER_IDENTITY" "$RECEIPT" &&
   (cd "$OUT_DIR" && sha256sum -c SHA256SUMS >/dev/null 2>&1); then
  echo "$OUT_DIR/mister_magik_scanout_slots.ko"
  exit 0
fi

build_commands=$(cat <<'EOS'
set -euo pipefail
if [[ ! -f "$KERNEL_BUILD/.config" ]]; then
  make -C "$KERNEL_SRC" O="$KERNEL_BUILD" ARCH=arm CROSS_COMPILE="$CROSS_COMPILE" LOCALVERSION="$LOCALVERSION" MiSTer_defconfig
fi
make -C "$KERNEL_SRC" O="$KERNEL_BUILD" ARCH=arm CROSS_COMPILE="$CROSS_COMPILE" LOCALVERSION="$LOCALVERSION" modules_prepare
make -C "$MODULE_DIR" KERNEL_SRC="$KERNEL_SRC" KERNEL_BUILD="$KERNEL_BUILD" ARCH=arm CROSS_COMPILE="$CROSS_COMPILE" LOCALVERSION="$LOCALVERSION" all
cp "$MODULE_DIR/mister_magik_scanout_slots.ko" "$OUT_DIR/mister_magik_scanout_slots.ko"
if command -v "${CROSS_COMPILE}modinfo" >/dev/null 2>&1; then
  "${CROSS_COMPILE}modinfo" "$OUT_DIR/mister_magik_scanout_slots.ko" > "$OUT_DIR/modinfo.txt"
elif command -v modinfo >/dev/null 2>&1; then
  modinfo "$OUT_DIR/mister_magik_scanout_slots.ko" > "$OUT_DIR/modinfo.txt"
else
  echo "modinfo is required to attest the module" >&2
  exit 1
fi
kernel_config_sha256=$(sha256sum "$KERNEL_BUILD/.config" | awk '{print $1}')
uapi_sha256=$(sha256sum "$MODULE_DIR/mister_magik_scanout_slots_uapi.h" | awk '{print $1}')
platform_contract_sha256=$(sha256sum "$MODULE_DIR/mister_magik_scanout_platform.h" | awk '{print $1}')
source_sha256=$(cd "$MODULE_DIR" && sha256sum mister_magik_scanout_slots.c mister_magik_scanout_slots_uapi.h mister_magik_scanout_platform.h mister_magik_scanout_policy.h Makefile | sha256sum | awk '{print $1}')
compiler_version=$("${CROSS_COMPILE}gcc" --version | sed -n '1p')
module_sha256=$(sha256sum "$OUT_DIR/mister_magik_scanout_slots.ko" | awk '{print $1}')
if command -v "${CROSS_COMPILE}nm" >/dev/null 2>&1; then
  "${CROSS_COMPILE}nm" -u "$OUT_DIR/mister_magik_scanout_slots.ko" > "$OUT_DIR/imports.txt"
elif command -v nm >/dev/null 2>&1; then
  nm -u "$OUT_DIR/mister_magik_scanout_slots.ko" > "$OUT_DIR/imports.txt"
else
  echo "nm is required to attest imported kernel symbols" >&2
  exit 1
fi
cat > "$OUT_DIR/provenance.txt" <<EOF
kernel_revision=$KERNEL_REVISION
kernel_config_sha256=$kernel_config_sha256
fb_driver_sha256=$PINNED_FB_DRIVER_SHA256
dt_sha256=$PINNED_DT_SHA256
uapi_sha256=$uapi_sha256
platform_contract_sha256=$platform_contract_sha256
source_sha256=$source_sha256
source_revision=$SOURCE_REVISION
source_dirty=$SOURCE_DIRTY
observed_source_revision=$OBSERVED_SOURCE_REVISION
observed_source_dirty=$OBSERVED_SOURCE_DIRTY
compiler=$compiler_version
module_sha256=$module_sha256
vermagic=$(sed -n 's/^vermagic:[[:space:]]*//p' "$OUT_DIR/modinfo.txt")
EOF
(cd "$OUT_DIR" && sha256sum mister_magik_scanout_slots.ko modinfo.txt provenance.txt imports.txt > SHA256SUMS)
EOS
)

if command -v "${CROSS_COMPILE}gcc" >/dev/null 2>&1; then
  KERNEL_SRC="$KERNEL_SRC" KERNEL_BUILD="$KERNEL_BUILD" MODULE_DIR="$MODULE_DIR" OUT_DIR="$OUT_DIR" KERNEL_REVISION="$KERNEL_REVISION" \
    PINNED_FB_DRIVER_SHA256="$PINNED_FB_DRIVER_SHA256" PINNED_DT_SHA256="$PINNED_DT_SHA256" SOURCE_REVISION="$SOURCE_REVISION" SOURCE_DIRTY="$SOURCE_DIRTY" \
    OBSERVED_SOURCE_REVISION="$OBSERVED_SOURCE_REVISION" OBSERVED_SOURCE_DIRTY="$OBSERVED_SOURCE_DIRTY" BUILD_IDENTITY="$BUILD_IDENTITY" RECEIPT="$RECEIPT" \
    CROSS_COMPILE="$CROSS_COMPILE" LOCALVERSION="$LOCALVERSION" IMAGE="$IMAGE" IMAGE_DIGEST="$IMAGE_DIGEST" \
    DOCKERFILE_SHA256="$DOCKERFILE_SHA256" COMPILER_IDENTITY="$COMPILER_IDENTITY" bash -lc "$build_commands"
else
  CONTAINER_KERNEL_SRC="/kernel-src"
  container run --arch arm64 --rm --cpus 8 --memory 8g \
    --volume "$ROOT:/project" \
    --volume "$KERNEL_SRC:$CONTAINER_KERNEL_SRC" \
    --workdir /project \
    "$IMAGE" bash -lc "
      set -euo pipefail
      export KERNEL_SRC='$CONTAINER_KERNEL_SRC'
      export KERNEL_BUILD='/project/build/scanout-slots-kernel'
      export MODULE_DIR='/project/mister/platform/kernel/scanout-slots'
      export OUT_DIR='/project/build/scanout-slots'
      export CROSS_COMPILE='$CROSS_COMPILE'
      export LOCALVERSION='$LOCALVERSION'
      export KERNEL_REVISION='$KERNEL_REVISION'
      export PINNED_FB_DRIVER_SHA256='$PINNED_FB_DRIVER_SHA256'
      export PINNED_DT_SHA256='$PINNED_DT_SHA256'
      export SOURCE_REVISION='$SOURCE_REVISION'
      export SOURCE_DIRTY='$SOURCE_DIRTY'
      export OBSERVED_SOURCE_REVISION='$OBSERVED_SOURCE_REVISION'
      export OBSERVED_SOURCE_DIRTY='$OBSERVED_SOURCE_DIRTY'
      $build_commands
    "
fi

COMPILER_IDENTITY="$(sed -n 's/^compiler=//p' "$OUT_DIR/provenance.txt")"
if [[ -z "$COMPILER_IDENTITY" ]]; then
  echo "built provenance is missing compiler identity" >&2
  exit 1
fi
BUILD_IDENTITY="$(compute_build_identity)"
printf 'receipt_version=2\nbuild_identity=%s\nimage_reference=%s\nimage_digest=%s\ndockerfile_sha256=%s\ncompiler_identity=%s\nkernel_config_sha256=%s\n' \
  "$BUILD_IDENTITY" "$IMAGE" "$IMAGE_DIGEST" "$DOCKERFILE_SHA256" "$COMPILER_IDENTITY" \
  "$(sha256sum "$KERNEL_BUILD/.config" | awk '{print $1}')" >"$RECEIPT"
echo "$OUT_DIR/mister_magik_scanout_slots.ko"
