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
PINNED_KERNEL_REVISION="f0fb626acadd07f0718934826b143b6e4c9ce81c"
PINNED_FB_DRIVER_SHA256="b85ccabd33c3360c60873eb29deb933500b117759c3a3e898637a3e46e25312c"
PINNED_DT_SHA256="36d7f660df55253a9ba11ebce615f304b91c3d7c99be94173af443574ad28a95"
KERNEL_REVISION="$(git -c safe.directory="$KERNEL_SRC" -C "$KERNEL_SRC" rev-parse --verify 'HEAD^{commit}')"
OBSERVED_SOURCE_REVISION="$(git -c safe.directory="$ROOT" -C "$ROOT" rev-parse --verify 'HEAD^{commit}')"
if [[ -z "$(git -c safe.directory="$ROOT" -C "$ROOT" status --porcelain --untracked-files=all)" ]]; then
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
echo "$OUT_DIR/mister_magik_scanout_slots.ko"
EOS
)

if command -v "${CROSS_COMPILE}gcc" >/dev/null 2>&1; then
  KERNEL_SRC="$KERNEL_SRC" KERNEL_BUILD="$KERNEL_BUILD" MODULE_DIR="$MODULE_DIR" OUT_DIR="$OUT_DIR" KERNEL_REVISION="$KERNEL_REVISION" \
    PINNED_FB_DRIVER_SHA256="$PINNED_FB_DRIVER_SHA256" PINNED_DT_SHA256="$PINNED_DT_SHA256" SOURCE_REVISION="$SOURCE_REVISION" SOURCE_DIRTY="$SOURCE_DIRTY" \
    OBSERVED_SOURCE_REVISION="$OBSERVED_SOURCE_REVISION" OBSERVED_SOURCE_DIRTY="$OBSERVED_SOURCE_DIRTY" \
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
    export KERNEL_REVISION='$KERNEL_REVISION'
    export PINNED_FB_DRIVER_SHA256='$PINNED_FB_DRIVER_SHA256'
    export PINNED_DT_SHA256='$PINNED_DT_SHA256'
    export SOURCE_REVISION='$SOURCE_REVISION'
    export SOURCE_DIRTY='$SOURCE_DIRTY'
    export OBSERVED_SOURCE_REVISION='$OBSERVED_SOURCE_REVISION'
    export OBSERVED_SOURCE_DIRTY='$OBSERVED_SOURCE_DIRTY'
    $build_commands
  "
