#!/usr/bin/env bash
# Install update_all-compatible Magik boot through MiSTer's native main= hook.
#
# Stock /media/fat/MiSTer stays as the only inittab menu entry. It reads
# MiSTer.ini and re-execs /media/fat/MiSTer_MagiK.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
source "$ROOT/scripts/reboot-wait-lib.sh"

echo "==> Configure device (stock inittab + MiSTer.ini main=MiSTer_MagiK)"
scripts/mister run '
set -e
if [ ! -x /media/fat/MiSTer_MagiK ]; then
  echo "ERROR: /media/fat/MiSTer_MagiK is missing or not executable"
  echo "Run scripts/deploy-main-mister-experiment.sh first."
  exit 1
fi
if [ ! -f /media/fat/mister-magik/experiments/menu-magik-vblank-latch.rbf ]; then
  echo "ERROR: production latch RBF is missing"
  echo "Run scripts/deploy-main-mister-experiment.sh to install the CI-built latch RBF."
  exit 1
fi
META=/media/fat/mister-magik/experiments/menu-magik-vblank-latch.metadata.txt
if [ ! -f "$META" ]; then
  echo "ERROR: production latch RBF metadata is missing"
  exit 1
fi
EXPECTED=$(sed -n "s/^rbf_sha256=//p" "$META")
ACTUAL=$(sha256sum /media/fat/mister-magik/experiments/menu-magik-vblank-latch.rbf | awk "{print \$1}")
if [ -z "$EXPECTED" ] || [ "$EXPECTED" != "$ACTUAL" ]; then
  echo "ERROR: production latch RBF does not match adjacent metadata"
  exit 1
fi
if [ ! -f /media/fat/mister-magik/mister_magik_scanout_slots.ko ]; then
  echo "ERROR: production scanout-slots module is missing"
  echo "Run scripts/deploy-main-mister-experiment.sh to install the plugin module."
  exit 1
fi
PLUGIN=/media/fat/mister-magik/mister_magik_scanout_slots.ko
PLUGIN_META=/media/fat/mister-magik/mister_magik_scanout_slots.metadata.txt
PINNED_KERNEL_REVISION=f0fb626acadd07f0718934826b143b6e4c9ce81c
PINNED_KERNEL_CONFIG_SHA256=7137fafad42831de17a46755f1bfcd3b56b6ac5922c1981a515ccd4ce6158068
PINNED_UAPI_SHA256=2d8dffc12b76c7346cbd6291ece440e824719ebb6b564192e3ba3f692eb8c5b9
PINNED_SOURCE_SHA256=2bd6b3cc4bc4718cbe7db18f88e10c0f5a37585821a9c47075ea4c65adef92fc
if [ ! -f "$PLUGIN_META" ]; then
  echo "ERROR: production scanout-slots provenance is missing"
  exit 1
fi
PLUGIN_EXPECTED=$(sed -n "s/^module_sha256=//p" "$PLUGIN_META")
PLUGIN_ACTUAL=$(sha256sum "$PLUGIN" | awk "{print \$1}")
PLUGIN_VERMAGIC=$(sed -n "s/^vermagic=//p" "$PLUGIN_META")
case "$PLUGIN_VERMAGIC" in 5.15.1-MiSTer\ *) ;; *)
  echo "ERROR: production scanout-slots module targets the wrong kernel"
  exit 1
esac
if [ -z "$PLUGIN_EXPECTED" ] || [ "$PLUGIN_EXPECTED" != "$PLUGIN_ACTUAL" ]; then
  echo "ERROR: production scanout-slots module does not match adjacent provenance"
  exit 1
fi
if ! grep -q "^kernel_revision=$PINNED_KERNEL_REVISION$" "$PLUGIN_META" ||
   ! grep -q "^kernel_config_sha256=$PINNED_KERNEL_CONFIG_SHA256$" "$PLUGIN_META" ||
   ! grep -q "^uapi_sha256=$PINNED_UAPI_SHA256$" "$PLUGIN_META" ||
   ! grep -q "^source_sha256=$PINNED_SOURCE_SHA256$" "$PLUGIN_META" ||
   ! grep -q "^compiler=arm-linux-gnueabihf-gcc (Ubuntu 9\.4\.0" "$PLUGIN_META"; then
  echo "ERROR: production scanout-slots provenance is not the qualified build"
  exit 1
fi
PLUGIN_RBF_SHA256=$(sed -n "s/^rbf_sha256=//p" "$PLUGIN_META")
if [ "$PLUGIN_RBF_SHA256" != "$EXPECTED" ]; then
  echo "ERROR: scanout-slots module and latch RBF are not a qualified pair"
  exit 1
fi

INI=/media/fat/MiSTer.ini
STAMP=$(date +%Y%m%d-%H%M%S 2>/dev/null || echo unknown)
SNAP="/media/fat/mister-magik/snapshots/$STAMP-install"
mkdir -p "$SNAP"
cp /etc/inittab "$SNAP/inittab" 2>/dev/null || true
cp "$INI" "$SNAP/MiSTer.ini" 2>/dev/null || true
ps > "$SNAP/ps.txt" 2>/dev/null || true
cat /sys/module/MiSTer_fb/parameters/mode > "$SNAP/fb-mode.txt" 2>/dev/null || true
cp /tmp/mister-magik-main.log "$SNAP/mister-magik-main.log" 2>/dev/null || true
echo "snapshot: $SNAP"
if [ ! -f "$INI.bak" ]; then cp "$INI" "$INI.bak"; fi

echo "=== post-install processes ==="
ps | grep -E "[M]iSTer|[M]iSTer_MagiK|[m]ister-magik-fb" || true
echo "=== post-install fb mode ==="
cat /sys/module/MiSTer_fb/parameters/mode 2>/dev/null || true
'

scripts/mister inittab-ensure-stock
scripts/mister ini-repair-boot
scripts/mister ini-repair-arcade-video

mister_reboot_wait_with_raw_fallback

echo "Done. Stock MiSTer should hand off to MiSTer_MagiK via MiSTer.ini main=."
