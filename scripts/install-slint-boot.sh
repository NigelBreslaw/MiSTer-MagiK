#!/usr/bin/env bash
# Enable update_all-safe MagiK boot after verifying the installed platform bundle.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
source "$ROOT/scripts/reboot-wait-lib.sh"

echo "==> Verify installed production platform"
scripts/mister run '
set -e
manifest=/media/fat/mister-magik/platform-v1.manifest
get() {
  value=$(sed -n "s/^$1=//p" "$manifest")
  test -n "$value"
  test "$(grep -c "^$1=" "$manifest")" -eq 1
  printf "%s" "$value"
}
test -r "$manifest"
test "$(get format)" = mister-magik-platform-v1
test "$(get main_path)" = /media/fat/MiSTer_MagiK
test "$(get gui_path)" = /media/fat/mister-magik/mister-magik-fb
test "$(get scanout_module_path)" = /media/fat/mister-magik/mister_magik_scanout_slots.ko
test "$(get scanout_metadata_path)" = /media/fat/mister-magik/mister_magik_scanout_slots.metadata.txt
test "$(get latch_rbf_path)" = /media/fat/mister-magik/fpga/menu-magik-vblank-latch.rbf
test "$(get latch_metadata_path)" = /media/fat/mister-magik/fpga/menu-magik-vblank-latch.metadata.txt
check() {
  path=$(get "$1_path")
  expected=$(get "$1_sha256")
  test -r "$path"
  test "$(sha256sum "$path" | awk "{print \$1}")" = "$expected"
}
check main
check gui
check scanout_module
check scanout_metadata
check latch_rbf
check latch_metadata
contract=$(get platform_contract_sha256)
menu_revision=$(get menu_revision)
module_hash=$(get scanout_module_sha256)
rbf_hash=$(get latch_rbf_sha256)
module_meta=$(get scanout_metadata_path)
rbf_meta=$(get latch_metadata_path)
grep -qx "platform_contract_sha256=$contract" "$module_meta"
grep -qx "platform_contract_sha256=$contract" "$rbf_meta"
grep -qx "module_sha256=$module_hash" "$module_meta"
grep -qx "rbf_sha256=$rbf_hash" "$rbf_meta"
grep -qx "source_commit=$menu_revision" "$rbf_meta"
case "$(sed -n "s/^vermagic=//p" "$module_meta")" in 5.15.1-MiSTer\ *) ;; *) exit 1;; esac
INI=/media/fat/MiSTer.ini
STAMP=$(date +%Y%m%d-%H%M%S 2>/dev/null || echo unknown)
SNAP="/media/fat/mister-magik/snapshots/$STAMP-install"
mkdir -p "$SNAP"
cp /etc/inittab "$SNAP/inittab" 2>/dev/null || true
cp "$INI" "$SNAP/MiSTer.ini" 2>/dev/null || true
cp "$manifest" "$SNAP/platform-v1.manifest"
if [ ! -f "$INI.bak" ]; then cp "$INI" "$INI.bak"; fi
echo "snapshot: $SNAP"
'

echo "==> Configure stock inittab + MiSTer.ini main=MiSTer_MagiK"
scripts/mister inittab-ensure-stock
scripts/mister ini-repair-boot
scripts/mister ini-repair-arcade-video

# Settings and boot ownership are changing, so use a normal bounded reboot.
mister_reboot_wait_with_raw_fallback
echo "Done. Root menu.rbf remains stock; MiSTer_MagiK owns the production latch RBF."
