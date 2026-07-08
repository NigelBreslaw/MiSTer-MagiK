#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
MISTER="$ROOT/scripts/mister"
REMOTE_DIR="/tmp/mister-magik-plugin-probe"
REMOTE_KO="$REMOTE_DIR/mister_magik_plugin_probe.ko"
REMOTE_BIN="/media/fat/mister-magik/mister-magik-fb"
LOCAL_KO="$ROOT/build/plugin-probe/mister_magik_plugin_probe.ko"
LOCAL_REPORT="$ROOT/build/plugin-probe/plugin-map-report.log"
LOCAL_BANDWIDTH="$ROOT/build/plugin-probe/plugin-map-bandwidth.log"
LOCAL_PATTERN="$ROOT/build/plugin-probe/plugin-present-pattern.log"
FRAMES="${MISTER_PLUGIN_PROBE_FRAMES:-120}"
PATTERN_FRAMES="${MISTER_PLUGIN_PRESENT_PATTERN_FRAMES:-180}"
SCROLL_SECS="${MISTER_PLUGIN_LAUNCHER_SCROLL_SECS:-15}"
SCROLL_LABEL="${MISTER_PLUGIN_LAUNCHER_LABEL:-PLUGIN-MAIN-VSYNC-$(date -u +%Y%m%dT%H%M%SZ)}"

cleanup() {
  "$MISTER" run "rmmod mister_magik_plugin_probe 2>/dev/null || true; rm -rf '$REMOTE_DIR'" >/dev/null 2>&1 || true
}
trap cleanup EXIT

mkdir -p "$ROOT/build/plugin-probe"

"$ROOT/scripts/build-plugin-probe-module.sh"
"$ROOT/magik-gui/build-arm.sh" --diagnostics --bench-tools

test -f "$LOCAL_KO"
if ! grep -q 'vermagic:.*5\.15\.1-MiSTer' "$ROOT/build/plugin-probe/modinfo.txt"; then
  echo "module vermagic does not target 5.15.1-MiSTer:" >&2
  cat "$ROOT/build/plugin-probe/modinfo.txt" >&2
  exit 1
fi

echo "==> Verifying stock kernel and module tools"
"$MISTER" run "set -e; uname -r; test \"\$(uname -r)\" = '5.15.1-MiSTer'; command -v insmod; command -v rmmod; command -v modprobe >/dev/null"

echo "==> Clearing any old probe module"
cleanup

echo "==> Uploading diagnostics binary and probe module"
"$MISTER" run "pidof mister-magik-fb 2>/dev/null | xargs -r kill -9; mkdir -p /media/fat/mister-magik '$REMOTE_DIR'"
"$MISTER" put "$ROOT/magik-gui/target/armv7-unknown-linux-gnueabihf/release-device/mister-magik-fb" "$REMOTE_BIN"
"$MISTER" put "$LOCAL_KO" "$REMOTE_KO"
"$MISTER" run "chmod +x '$REMOTE_BIN'; chmod 600 '$REMOTE_KO'; sync"

echo "==> Loading module"
"$MISTER" run "insmod '$REMOTE_KO'; test -e /dev/mister-magik-plugin-probe; grep '^mister_magik_plugin_probe ' /proc/modules"

echo "==> Running plugin-map-report"
"$MISTER" run "'$REMOTE_BIN' plugin-map-report" | tee "$LOCAL_REPORT"

echo "==> Running plugin-map-bandwidth ($FRAMES frames)"
"$MISTER" run "'$REMOTE_BIN' plugin-map-bandwidth '$FRAMES'" | tee "$LOCAL_BANDWIDTH"

echo "==> Starting launcher for plugin present pattern diagnostic"
"$ROOT/scripts/run-rust.sh" launcher 0
"$MISTER" run "set -e; for i in \$(seq 1 20); do pidof mister-magik-fb >/dev/null 2>&1 && exit 0; sleep 0.5; done; exit 1"

echo "==> Running plugin-present-pattern ($PATTERN_FRAMES frames)"
"$MISTER" run "'$REMOTE_BIN' plugin-present-pattern '$PATTERN_FRAMES'" | tee "$LOCAL_PATTERN"

echo "==> Running plugin-backed launcher scroll profile ($SCROLL_LABEL, ${SCROLL_SECS}s)"
MISTER_PRESENT_BACKEND=plugin-main-vsync-hidden \
"$ROOT/scripts/profile-arcade-scroll.sh" "$SCROLL_LABEL" --skip-build --secs "$SCROLL_SECS" --scenario turbo-hold --skip-boot-prelude --catalog-refresh off --stream-consumer none

echo "==> Unloading module and restoring normal MagiK"
cleanup
"$ROOT/scripts/deploy-rust.sh"
"$ROOT/scripts/run-rust.sh" launcher 0
"$MISTER" run "set -e; for i in \$(seq 1 20); do pidof mister-magik-fb >/dev/null 2>&1 && break; sleep 0.5; done; ! test -e /dev/mister-magik-plugin-probe; ! grep -q '^mister_magik_plugin_probe ' /proc/modules; pidof MiSTer_MagiK; pidof mister-magik-fb; ls -l /media/fat/mister-magik/launcher.env /tmp/mister-magik/fs-fault* /media/fat/mister-magik/rebuild-on-next-boot 2>/dev/null || true"

echo "==> Wrote:"
echo "    $LOCAL_REPORT"
echo "    $LOCAL_BANDWIDTH"
echo "    $LOCAL_PATTERN"
echo "    $ROOT/build/arcade-scroll-profiles/${SCROLL_LABEL}-arcade-scroll.tsv"
