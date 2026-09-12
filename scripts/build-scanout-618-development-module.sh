#!/usr/bin/env bash
# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
KERNEL_SRC="${KERNEL_SRC:-$ROOT/../Linux-Kernel_MiSTer}"
KERNEL_BUILD="${KERNEL_BUILD:-$KERNEL_SRC}"
OUT_DIR="${OUT_DIR:-$ROOT/build/scanout-618-development}"
CROSS_COMPILE="${CROSS_COMPILE:-arm-none-linux-gnueabihf-}"
LOCALVERSION="${LOCALVERSION:--MiSTer}"
KERNEL_REVISION=6a581bac47c32dfd2525f9874fd263cf08058610
KERNEL_RELEASE=6.18.38-MiSTer
KERNEL_CONFIG_SHA256=584c7fdb7884616363b38c0514266a5fc40083ae327d9a71e72deb6f3101cdab
MODULE_SYMVERS_SHA256=f58b220d8cdcb925afdd4ba4a4c0a04c02154a8f2fc658cc1fa885b89f79952f
PROVIDER_IDENTITY=stock-6.18-latch-reuse-v3
VERMAGIC='6.18.38-MiSTer SMP mod_unload ARMv7 p2v8 '
MODULE_DIR="$ROOT/mister/platform/kernel/scanout-618"

usage() {
  cat <<'EOF'
Usage: scripts/build-scanout-618-development-module.sh

Build the development-only fixed-window provider against the exact prepared
MiSTer Linux 6.18 tree. KERNEL_SRC and KERNEL_BUILD may select separate source
and output trees; both identities remain fail-closed.
EOF
}

case "${1:-}" in
  -h|--help) usage; exit 0 ;;
  "") ;;
  *) echo "unknown argument: $1" >&2; usage >&2; exit 2 ;;
esac

for required in "$KERNEL_SRC/.git" "$KERNEL_BUILD/.config" "$KERNEL_BUILD/Module.symvers"; do
  [[ -e "$required" ]] || { echo "missing exact kernel input: $required" >&2; exit 1; }
done
observed_revision="$(git -c safe.directory="$KERNEL_SRC" -C "$KERNEL_SRC" rev-parse HEAD)"
[[ "$observed_revision" == "$KERNEL_REVISION" ]] || {
  echo "kernel revision $observed_revision is not pinned $KERNEL_REVISION" >&2
  exit 1
}
echo "$KERNEL_CONFIG_SHA256  $KERNEL_BUILD/.config" | sha256sum -c -
echo "$MODULE_SYMVERS_SHA256  $KERNEL_BUILD/Module.symvers" | sha256sum -c -
command -v "${CROSS_COMPILE}gcc" >/dev/null
command -v "${CROSS_COMPILE}nm" >/dev/null
command -v modinfo >/dev/null

if [[ -d "$OUT_DIR" && -n "$(find "$OUT_DIR" -mindepth 1 -maxdepth 1 -print -quit)" ]]; then
  echo "output directory must be absent or empty: $OUT_DIR" >&2
  exit 1
fi
mkdir -p "$OUT_DIR"
make -C "$KERNEL_SRC" O="$KERNEL_BUILD" ARCH=arm \
  CROSS_COMPILE="$CROSS_COMPILE" LOCALVERSION="$LOCALVERSION" \
  M="$MODULE_DIR" MISTER_MAGIK_DEVELOPMENT_TRIAL=1 modules
module="$MODULE_DIR/mister_magik_scanout_slots.ko"
[[ -f "$module" ]]
cp "$module" "$OUT_DIR/mister_magik_scanout_slots.ko"
modinfo "$OUT_DIR/mister_magik_scanout_slots.ko" > "$OUT_DIR/modinfo.txt"
"${CROSS_COMPILE}nm" -u "$OUT_DIR/mister_magik_scanout_slots.ko" > "$OUT_DIR/imports.txt"
observed_vermagic="$(modinfo -F vermagic "$OUT_DIR/mister_magik_scanout_slots.ko")"
[[ "$observed_vermagic" == "$VERMAGIC" ]]
[[ "$(modinfo -F mister_magik_development_trial "$OUT_DIR/mister_magik_scanout_slots.ko")" == "$PROVIDER_IDENTITY" ]]
[[ "$(modinfo -F license "$OUT_DIR/mister_magik_scanout_slots.ko")" == GPL ]]
[[ -z "$(modinfo -F depends "$OUT_DIR/mister_magik_scanout_slots.ko")" ]]

module_sha256="$(sha256sum "$OUT_DIR/mister_magik_scanout_slots.ko" | awk '{print $1}')"
platform_contract_sha256="$(sha256sum \
  "$ROOT/mister/platform/kernel/scanout-slots/mister_magik_scanout_platform.h" | awk '{print $1}')"
source_revision="$(git -C "$ROOT" log -1 --format=%H -- \
  mister/platform/kernel/scanout-618 \
  mister/platform/kernel/main-window \
  mister/platform/kernel/scanout-slots/mister_magik_scanout_slots_uapi.h \
  scripts/build-scanout-618-development-module.sh)"
[[ -z "$(git -C "$ROOT" status --porcelain -- \
  mister/platform/kernel/scanout-618 \
  mister/platform/kernel/main-window \
  mister/platform/kernel/scanout-slots/mister_magik_scanout_slots_uapi.h \
  scripts/build-scanout-618-development-module.sh)" ]] || {
  echo "development provider inputs must be committed before attestation" >&2
  exit 1
}
cat > "$OUT_DIR/provenance.txt" <<EOF
kernel_release=$KERNEL_RELEASE
kernel_revision=$KERNEL_REVISION
kernel_config_sha256=$KERNEL_CONFIG_SHA256
module_symvers_sha256=$MODULE_SYMVERS_SHA256
platform_profile=$PROVIDER_IDENTITY
provider_identity=$PROVIDER_IDENTITY
development_only=1
platform_contract_sha256=$platform_contract_sha256
source_revision=$source_revision
source_dirty=0
compiler=$(${CROSS_COMPILE}gcc --version | sed -n '1p')
module_sha256=$module_sha256
vermagic=$VERMAGIC
EOF
(cd "$OUT_DIR" && sha256sum \
  mister_magik_scanout_slots.ko modinfo.txt provenance.txt imports.txt > SHA256SUMS)
