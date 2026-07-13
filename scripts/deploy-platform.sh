#!/usr/bin/env bash
# Build and atomically deploy the complete MiSTer MagiK production platform.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
GUI_DIR="$ROOT/magik-gui"
MAIN_DIR="${MISTER_MAIN_DIR:-$ROOT/../Main_MiSTer}"
CLEAN_MAIN=0

usage() {
  cat <<'EOF'
Usage: scripts/deploy-platform.sh [--clean-main]

Builds Main and the ARM frontend, verifies the prebuilt qualified FPGA/module
artifacts, uploads every file with a temporary suffix, and activates
platform-v1.manifest last. It never writes /media/fat/menu.rbf and never
reboots the device.
EOF
}

for arg in "$@"; do
  case "$arg" in
    --clean-main) CLEAN_MAIN=1 ;;
    -h|--help) usage; exit 0 ;;
    *) echo "ERROR: unknown argument: $arg" >&2; exit 2 ;;
  esac
done

GUI_BIN="$GUI_DIR/target/armv7-unknown-linux-gnueabihf/release-device/mister-magik-fb"
CATALOG_BUILDER="$GUI_DIR/target/armv7-unknown-linux-gnueabihf/release-device/mister-magik-catalog-builder"
MAIN_BIN="$MAIN_DIR/bin/MiSTer"
MODULE="$ROOT/build/scanout-slots/mister_magik_scanout_slots.ko"
MODULE_META="$ROOT/build/scanout-slots/provenance.txt"
RBF="$ROOT/build/fpga-vblank-latch/menu-magik-vblank-latch.rbf"
RBF_META="$ROOT/build/fpga-vblank-latch/menu-magik-vblank-latch.metadata.txt"
MANIFEST="$ROOT/build/platform-v1.manifest"

if [[ ! -d "$MAIN_DIR" || ! -x "$MAIN_DIR/build-container.sh" ]]; then
  echo "ERROR: Main_MiSTer fork checkout not found: $MAIN_DIR" >&2
  exit 1
fi
if [[ -n "$(git -C "$MAIN_DIR" status --porcelain --untracked-files=all)" ]]; then
  echo "ERROR: Main_MiSTer must be clean before a production deployment" >&2
  exit 1
fi
if [[ -n "$(git -C "$ROOT" status --porcelain --untracked-files=no)" ]]; then
  echo "ERROR: MiSTer MagiK tracked files must be clean before a production deployment" >&2
  exit 1
fi
for artifact in "$MODULE" "$MODULE_META" "$RBF" "$RBF_META"; do
  if [[ ! -f "$artifact" ]]; then
    echo "ERROR: missing qualified artifact: $artifact" >&2
    exit 1
  fi
done
"$ROOT/scripts/verify-fpga-rbf-manifest.py" "$RBF_META" >/dev/null

echo "==> Building magik-gui production binary"
"$GUI_DIR/build-arm.sh" --device
echo "==> Building Main fork"
if [[ "$CLEAN_MAIN" == 1 ]]; then "$MAIN_DIR/build-container.sh" clean; fi
"$MAIN_DIR/build-container.sh"

MAIN_REVISION="$(git -C "$MAIN_DIR" rev-parse HEAD)"
MAGIK_REVISION="$(git -C "$ROOT" rev-parse HEAD)"
"$ROOT/scripts/platform-manifest.py" generate \
  --output "$MANIFEST" \
  --main "$MAIN_BIN" \
  --gui "$GUI_BIN" \
  --catalog-builder "$CATALOG_BUILDER" \
  --scanout-module "$MODULE" \
  --scanout-metadata "$MODULE_META" \
  --latch-rbf "$RBF" \
  --latch-metadata "$RBF_META" \
  --main-revision "$MAIN_REVISION" \
  --magik-revision "$MAGIK_REVISION" >/dev/null

echo "==> Snapshotting and suspending the active launcher"
"$ROOT/scripts/mister" run '
set -e
STAMP=$(date +%Y%m%d-%H%M%S 2>/dev/null || echo unknown)
SNAP="/media/fat/mister-magik/snapshots/$STAMP-deploy"
mkdir -p "$SNAP" /media/fat/mister-magik/fpga
cp /etc/inittab "$SNAP/inittab" 2>/dev/null || true
cp /media/fat/MiSTer.ini "$SNAP/MiSTer.ini" 2>/dev/null || true
cp /media/fat/mister-magik/platform-v1.manifest "$SNAP/platform-v1.manifest" 2>/dev/null || true
if [ -p /dev/MiSTer_cmd ] && pidof MiSTer_MagiK >/dev/null 2>&1; then
  printf "mister_magik_suspend\n" > /dev/MiSTer_cmd
  sleep 1
fi
echo "snapshot: $SNAP"
'

declare -a LOCAL=("$GUI_BIN" "$CATALOG_BUILDER" "$MAIN_BIN" "$MODULE" "$MODULE_META" "$RBF" "$RBF_META" "$MANIFEST")
declare -a REMOTE=(
  /media/fat/mister-magik/mister-magik-fb
  /media/fat/mister-magik/mister-magik-catalog-builder
  /media/fat/MiSTer_MagiK
  /media/fat/mister-magik/mister_magik_scanout_slots.ko
  /media/fat/mister-magik/mister_magik_scanout_slots.metadata.txt
  /media/fat/mister-magik/fpga/menu-magik-vblank-latch.rbf
  /media/fat/mister-magik/fpga/menu-magik-vblank-latch.metadata.txt
  /media/fat/mister-magik/platform-v1.manifest
)
echo "==> Uploading inactive production bundle"
for index in "${!LOCAL[@]}"; do
  "$ROOT/scripts/mister" put "${LOCAL[$index]}" "${REMOTE[$index]}.upload"
done

echo "==> Verifying inactive bundle and activating manifest last"
"$ROOT/scripts/mister" run '
set -e
manifest=/media/fat/mister-magik/platform-v1.manifest.upload
get() { sed -n "s/^$1=//p" "$manifest"; }
test "$(get format)" = mister-magik-platform-v1
test "$(get main_path)" = /media/fat/MiSTer_MagiK
test "$(get gui_path)" = /media/fat/mister-magik/mister-magik-fb
test "$(get catalog_builder_path)" = /media/fat/mister-magik/mister-magik-catalog-builder
test "$(get scanout_module_path)" = /media/fat/mister-magik/mister_magik_scanout_slots.ko
test "$(get scanout_metadata_path)" = /media/fat/mister-magik/mister_magik_scanout_slots.metadata.txt
test "$(get latch_rbf_path)" = /media/fat/mister-magik/fpga/menu-magik-vblank-latch.rbf
test "$(get latch_metadata_path)" = /media/fat/mister-magik/fpga/menu-magik-vblank-latch.metadata.txt
check() { test "$(sha256sum "$1.upload" | awk "{print \$1}")" = "$(get "$2")"; }
check /media/fat/MiSTer_MagiK main_sha256
check /media/fat/mister-magik/mister-magik-fb gui_sha256
check /media/fat/mister-magik/mister-magik-catalog-builder catalog_builder_sha256
check /media/fat/mister-magik/mister_magik_scanout_slots.ko scanout_module_sha256
check /media/fat/mister-magik/mister_magik_scanout_slots.metadata.txt scanout_metadata_sha256
check /media/fat/mister-magik/fpga/menu-magik-vblank-latch.rbf latch_rbf_sha256
check /media/fat/mister-magik/fpga/menu-magik-vblank-latch.metadata.txt latch_metadata_sha256
contract=$(get platform_contract_sha256)
menu_revision=$(get menu_revision)
module_hash=$(get scanout_module_sha256)
rbf_hash=$(get latch_rbf_sha256)
grep -qx "platform_contract_sha256=$contract" /media/fat/mister-magik/mister_magik_scanout_slots.metadata.txt.upload
grep -qx "platform_contract_sha256=$contract" /media/fat/mister-magik/fpga/menu-magik-vblank-latch.metadata.txt.upload
grep -qx "module_sha256=$module_hash" /media/fat/mister-magik/mister_magik_scanout_slots.metadata.txt.upload
grep -qx "rbf_sha256=$rbf_hash" /media/fat/mister-magik/fpga/menu-magik-vblank-latch.metadata.txt.upload
grep -qx "source_commit=$menu_revision" /media/fat/mister-magik/fpga/menu-magik-vblank-latch.metadata.txt.upload
mv /media/fat/mister-magik/mister-magik-fb.upload /media/fat/mister-magik/mister-magik-fb
mv /media/fat/mister-magik/mister-magik-catalog-builder.upload /media/fat/mister-magik/mister-magik-catalog-builder
mv /media/fat/MiSTer_MagiK.upload /media/fat/MiSTer_MagiK
mv /media/fat/mister-magik/mister_magik_scanout_slots.ko.upload /media/fat/mister-magik/mister_magik_scanout_slots.ko
mv /media/fat/mister-magik/mister_magik_scanout_slots.metadata.txt.upload /media/fat/mister-magik/mister_magik_scanout_slots.metadata.txt
mv /media/fat/mister-magik/fpga/menu-magik-vblank-latch.rbf.upload /media/fat/mister-magik/fpga/menu-magik-vblank-latch.rbf
mv /media/fat/mister-magik/fpga/menu-magik-vblank-latch.metadata.txt.upload /media/fat/mister-magik/fpga/menu-magik-vblank-latch.metadata.txt
chmod 755 /media/fat/MiSTer_MagiK /media/fat/mister-magik/mister-magik-fb /media/fat/mister-magik/mister-magik-catalog-builder
chmod 600 /media/fat/mister-magik/mister_magik_scanout_slots.ko /media/fat/mister-magik/mister_magik_scanout_slots.metadata.txt /media/fat/mister-magik/fpga/menu-magik-vblank-latch.rbf /media/fat/mister-magik/fpga/menu-magik-vblank-latch.metadata.txt
sync
mv "$manifest" /media/fat/mister-magik/platform-v1.manifest
sync
'

echo "==> Enabling stock inittab + MiSTer.ini main=MiSTer_MagiK (no reboot)"
"$ROOT/scripts/mister" inittab-ensure-stock
"$ROOT/scripts/mister" ini-repair-boot
"$ROOT/scripts/mister" ini-repair-arcade-video
echo "Installed complete platform bundle; /media/fat/menu.rbf was not modified."
