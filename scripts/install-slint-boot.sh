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
PINNED_FB_DRIVER_SHA256=b85ccabd33c3360c60873eb29deb933500b117759c3a3e898637a3e46e25312c
PINNED_DT_SHA256=36d7f660df55253a9ba11ebce615f304b91c3d7c99be94173af443574ad28a95
PINNED_MAIN_REVISION=2fe8f7cdbe18bb7eab1b8f7baef74fd4b8ba66c1
PINNED_MAGIK_REVISION=4e08fb4e8125f865d10167d4c9d3fd87815f4f11
PINNED_MENU_REVISION=cf4dfdee516fcaa6952bdd9fb47154e96c28567e
PINNED_FPGA_PATCH_SHA256=4bdd2bcee724bb988ab6a975c2532ccc39a4e2b5686fac6fe4c88528f9c55ba6
PINNED_LATCH_RTL_SHA256=b810de3fdffbe79b8496e7eaa3967b07f6aa70a3d78dabb41c6428d72d994b1a
PINNED_RBF_SHA256=69e0e312b226c004bfe7fced2cc1145954efa1110cee7a0f58de1528d52627a1
PINNED_FPGA_BUILD_DATE=260711
PINNED_PLATFORM_CONTRACT_SHA256=8481c082c327c2892bf0fd64a68195472d4336723f6ab467a611655f949b1faf
PINNED_SOURCE_SHA256=b2f0aa6cf9db39c064b15d6ec735d0930fd4c9975fcae42ede07aeaf3c6f435b
if ! grep -q "^platform_contract_sha256=$PINNED_PLATFORM_CONTRACT_SHA256$" "$META"; then
  echo "ERROR: production latch RBF does not match the qualified platform contract"
  exit 1
fi
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
   ! grep -q "^fb_driver_sha256=$PINNED_FB_DRIVER_SHA256$" "$PLUGIN_META" ||
   ! grep -q "^dt_sha256=$PINNED_DT_SHA256$" "$PLUGIN_META" ||
   ! grep -q "^platform_contract_sha256=$PINNED_PLATFORM_CONTRACT_SHA256$" "$PLUGIN_META" ||
   ! grep -q "^source_sha256=$PINNED_SOURCE_SHA256$" "$PLUGIN_META" ||
   ! grep -q "^source_revision=[0-9a-f]\{40\}$" "$PLUGIN_META" ||
   ! grep -q "^observed_source_revision=[0-9a-f]\{40\}$" "$PLUGIN_META" ||
   ! grep -q "^source_dirty=0$" "$PLUGIN_META" ||
   ! grep -q "^observed_source_dirty=0$" "$PLUGIN_META" ||
   ! grep -q "^compiler=arm-linux-gnueabihf-gcc (Ubuntu 9\.4\.0" "$PLUGIN_META"; then
  echo "ERROR: production scanout-slots provenance is not the qualified build"
  exit 1
fi
if [ "$(sed -n "s/^source_revision=//p" "$PLUGIN_META")" != \
     "$(sed -n "s/^observed_source_revision=//p" "$PLUGIN_META")" ]; then
  echo "ERROR: production scanout-slots source revision is not the observed checkout"
  exit 1
fi
if ! grep -q "^main_revision=$PINNED_MAIN_REVISION$" "$PLUGIN_META" ||
   ! grep -q "^magik_revision=$PINNED_MAGIK_REVISION$" "$PLUGIN_META" ||
   ! grep -q "^menu_revision=$PINNED_MENU_REVISION$" "$PLUGIN_META" ||
   ! grep -q "^fpga_patch_sha256=$PINNED_FPGA_PATCH_SHA256$" "$PLUGIN_META" ||
   ! grep -q "^latch_rtl_sha256=$PINNED_LATCH_RTL_SHA256$" "$PLUGIN_META" ||
   ! grep -q "^fpga_build_date=$PINNED_FPGA_BUILD_DATE$" "$PLUGIN_META" ||
   ! grep -q "^rbf_sha256=$PINNED_RBF_SHA256$" "$PLUGIN_META"; then
  echo "ERROR: production scanout-slots artifacts are not the qualified platform set"
  exit 1
fi
MAIN_EXPECTED=$(sed -n "s/^main_binary_sha256=//p" "$PLUGIN_META")
MAIN_ACTUAL=$(sha256sum /media/fat/MiSTer_MagiK | awk "{print \$1}")
if [ "${#MAIN_EXPECTED}" -ne 64 ]; then
  echo "ERROR: production Main binary hash is missing or invalid"
  exit 1
fi
case "$MAIN_EXPECTED" in *[!0-9a-f]*)
  echo "ERROR: production Main binary hash is missing or invalid"
  exit 1
esac
if [ "$MAIN_EXPECTED" != "$MAIN_ACTUAL" ]; then
  echo "ERROR: production Main binary does not match deployment provenance"
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
