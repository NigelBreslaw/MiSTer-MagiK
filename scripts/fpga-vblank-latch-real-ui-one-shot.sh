#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
MISTER="$ROOT/scripts/mister"
REMOTE_DIR="/tmp/mister-magik-plugin-probe"
REMOTE_KO="$REMOTE_DIR/mister_magik_plugin_probe.ko"
REMOTE_BIN="/media/fat/mister-magik/mister-magik-fb"
REMOTE_RBF="/media/fat/mister-magik/experiments/menu-magik-vblank-latch.rbf"
LOCAL_KO="$ROOT/build/plugin-probe/mister_magik_plugin_probe.ko"
LOCAL_RBF="$ROOT/build/fpga-vblank-latch/menu-magik-vblank-latch.rbf"
LOCAL_DIR="$ROOT/build/fpga-vblank-latch"
LABEL="${MISTER_FPGA_LATCH_LABEL:-FPGA-LATCH-$(date -u +%Y%m%dT%H%M%SZ)}"
PATTERN_FRAMES="${MISTER_FPGA_LATCH_PATTERN_FRAMES:-180}"
SCROLL_SECS="${MISTER_FPGA_LATCH_SCROLL_SECS:-15}"
CAPTURE="${MISTER_FPGA_LATCH_CAPTURE:-0}"

usage() {
  cat <<'EOF'
Usage:
  scripts/fpga-vblank-latch-real-ui-one-shot.sh

Builds/deploys the diagnostics binary, plugin probe module, and experimental
vblank-latched Menu RBF; loads the RBF once through Main's command path; runs
FPGA latch diagnostics; then profiles the real launcher with
MISTER_PRESENT_BACKEND=fpga-vblank-latch-hidden.

Set MISTER_MENU_DIR to a writable Menu_MiSTer checkout if the RBF has not been
built yet. Set MISTER_FPGA_LATCH_CAPTURE=1 to record HDMI capture evidence.
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

mkdir -p "$LOCAL_DIR"

restore_normal() {
  set +e
  "$MISTER" run "rmmod mister_magik_plugin_probe 2>/dev/null || true; rm -rf '$REMOTE_DIR'" >/dev/null 2>&1
  "$MISTER" reboot-wait >/dev/null 2>&1
  "$ROOT/scripts/deploy-rust.sh" >/dev/null 2>&1
  "$ROOT/scripts/run-rust.sh" launcher 0 >/dev/null 2>&1
  "$MISTER" run "set -e; for i in \$(seq 1 20); do pidof mister-magik-fb >/dev/null 2>&1 && break; sleep 0.5; done; ! test -e /dev/mister-magik-plugin-probe; ! grep -q '^mister_magik_plugin_probe ' /proc/modules; pidof MiSTer_MagiK; pidof mister-magik-fb; ls -l /media/fat/mister-magik/launcher.env /tmp/mister-magik/fs-fault* /media/fat/mister-magik/rebuild-on-next-boot 2>/dev/null || true" >/dev/null 2>&1
  set -e
}
trap restore_normal EXIT

if [[ ! -f "$LOCAL_RBF" ]]; then
  "$ROOT/scripts/build-fpga-vblank-latch-core.sh"
fi
test -f "$LOCAL_RBF"

"$ROOT/scripts/build-plugin-probe-module.sh"
"$ROOT/magik-gui/build-arm.sh" --diagnostics --bench-tools

test -f "$LOCAL_KO"
if ! grep -q 'vermagic:.*5\.15\.1-MiSTer' "$ROOT/build/plugin-probe/modinfo.txt"; then
  echo "module vermagic does not target 5.15.1-MiSTer:" >&2
  cat "$ROOT/build/plugin-probe/modinfo.txt" >&2
  exit 1
fi

echo "==> Verifying stock runtime before one-shot experiment"
"$MISTER" status
"$MISTER" run "set -e; uname -r; test \"\$(uname -r)\" = '5.15.1-MiSTer'; command -v insmod; command -v rmmod; test ! -e /dev/mister-magik-plugin-probe || rmmod mister_magik_plugin_probe"

echo "==> Uploading diagnostics binary, plugin module, and experimental RBF"
"$MISTER" run "pidof mister-magik-fb 2>/dev/null | xargs -r kill -9; mkdir -p /media/fat/mister-magik/experiments '$REMOTE_DIR'"
"$MISTER" put "$ROOT/magik-gui/target/armv7-unknown-linux-gnueabihf/release-device/mister-magik-fb" "$REMOTE_BIN"
"$MISTER" put "$LOCAL_KO" "$REMOTE_KO"
"$MISTER" put "$LOCAL_RBF" "$REMOTE_RBF"
"$MISTER" run "chmod +x '$REMOTE_BIN'; chmod 600 '$REMOTE_KO'; sync"

echo "==> Loading plugin module"
"$MISTER" run "insmod '$REMOTE_KO'; test -e /dev/mister-magik-plugin-probe; grep '^mister_magik_plugin_probe ' /proc/modules"

echo "==> Loading experimental Menu core through Main command path"
"$MISTER" run "printf 'load_core $REMOTE_RBF\n' > /dev/MiSTer_cmd; sleep 2"

echo "==> Running FPGA latch capability report"
"$MISTER" run "'$REMOTE_BIN' fpga-latch-report" | tee "$LOCAL_DIR/${LABEL}-fpga-latch-report.log"

echo "==> Running single-post latch report"
"$MISTER" run "'$REMOTE_BIN' fpga-latch-post-report" | tee "$LOCAL_DIR/${LABEL}-fpga-latch-post-report.log"

echo "==> Running visible latch pattern ($PATTERN_FRAMES frames)"
"$MISTER" run "'$REMOTE_BIN' fpga-latch-pattern '$PATTERN_FRAMES'" | tee "$LOCAL_DIR/${LABEL}-fpga-latch-pattern.log"

if [[ "$CAPTURE" == "1" ]]; then
  echo "==> Capturing real launcher scroll with FPGA latch backend"
  MISTER_PRESENT_BACKEND=fpga-vblank-latch-hidden \
    "$ROOT/scripts/capture-arcade-scroll-video.sh" "$LABEL" --secs "$SCROLL_SECS" --capture-secs "$((SCROLL_SECS + 8))" --fps 25
else
  echo "==> Profiling real launcher scroll with FPGA latch backend"
  MISTER_PRESENT_BACKEND=fpga-vblank-latch-hidden \
    "$ROOT/scripts/profile-arcade-scroll.sh" "$LABEL" --skip-build --secs "$SCROLL_SECS" --scenario turbo-hold --skip-boot-prelude --catalog-refresh off --stream-consumer none
fi

echo "==> Restoring normal stock runtime"
restore_normal
trap - EXIT

echo "==> Wrote:"
echo "    $LOCAL_DIR/${LABEL}-fpga-latch-report.log"
echo "    $LOCAL_DIR/${LABEL}-fpga-latch-post-report.log"
echo "    $LOCAL_DIR/${LABEL}-fpga-latch-pattern.log"
echo "    $ROOT/build/arcade-scroll-profiles/${LABEL}-arcade-scroll.tsv"
if [[ "$CAPTURE" == "1" ]]; then
  echo "    $ROOT/build/arcade-scroll-captures/${LABEL}.mov"
fi
