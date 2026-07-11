#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
MISTER="$ROOT/scripts/mister"
REMOTE_DIR="/tmp/mister-magik-scanout-slots"
REMOTE_KO="$REMOTE_DIR/mister_magik_scanout_slots.ko"
REMOTE_BIN="/media/fat/mister-magik/mister-magik-fb"
REMOTE_RBF="/media/fat/mister-magik/experiments/menu-magik-vblank-latch.rbf"
REMOTE_ENV="/media/fat/mister-magik/launcher.env"
LOCAL_KO="$ROOT/build/plugin-probe/mister_magik_scanout_slots.ko"
LOCAL_RBF="$ROOT/build/fpga-vblank-latch/menu-magik-vblank-latch.rbf"
LOCAL_DIR="$ROOT/build/fpga-vblank-latch"
LABEL="${MISTER_FPGA_LATCH_LABEL:-FPGA-LATCH-$(date -u +%Y%m%dT%H%M%SZ)}"
PATTERN_FRAMES="${MISTER_FPGA_LATCH_PATTERN_FRAMES:-180}"
SCROLL_SECS="${MISTER_FPGA_LATCH_SCROLL_SECS:-15}"
CAPTURE="${MISTER_FPGA_LATCH_CAPTURE:-0}"

usage() {
  cat <<'EOF'
Usage:
  scripts/fpga-vblank-latch-real-ui-one-shot.sh [--capture] [--label LABEL] [--scroll-secs N]

Deploys the diagnostics binary, plugin probe module, and prebuilt experimental
vblank-latched Menu RBF; loads the RBF once through Main's MagiK launch command;
runs FPGA latch diagnostics; then profiles the real launcher with
MISTER_PRESENT_BACKEND=fpga-vblank-latch-hidden. The RBF must already exist at
build/fpga-vblank-latch/menu-magik-vblank-latch.rbf from the manual CI workflow;
this script does not build Quartus locally. Existing local plugin artifacts are
reused; the Rust diagnostics binary is rebuilt because normal deploys overwrite
the same target path.

Use --capture or MISTER_FPGA_LATCH_CAPTURE=1 to record HDMI capture evidence for
the Home-row repeat-hold pan.
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --capture) CAPTURE=1; shift ;;
    --label) LABEL="${2:?}"; shift 2 ;;
    --scroll-secs) SCROLL_SECS="${2:?}"; shift 2 ;;
    -h|--help) usage; exit 0 ;;
    *) echo "unknown argument: $1" >&2; usage >&2; exit 2 ;;
  esac
done

mkdir -p "$LOCAL_DIR"

restore_normal() {
  set +e
  "$MISTER" run "rm -f '$REMOTE_ENV'; rmmod mister_magik_scanout_slots 2>/dev/null || true; rm -rf '$REMOTE_DIR'" >/dev/null 2>&1
  "$MISTER" reboot-wait >/dev/null 2>&1
  "$ROOT/scripts/deploy-rust.sh" >/dev/null 2>&1
  "$ROOT/scripts/run-rust.sh" launcher 0 >/dev/null 2>&1
  "$MISTER" run "set -e; for i in \$(seq 1 20); do pidof mister-magik-fb >/dev/null 2>&1 && break; sleep 0.5; done; ! test -e /dev/mister-magik-scanout-slots; ! grep -q '^mister_magik_scanout_slots ' /proc/modules; pidof MiSTer_MagiK; pidof mister-magik-fb; ls -l /media/fat/mister-magik/launcher.env /tmp/mister-magik/fs-fault* /media/fat/mister-magik/rebuild-on-next-boot 2>/dev/null || true" >/dev/null 2>&1
  set -e
}
trap restore_normal EXIT

if [[ ! -f "$LOCAL_RBF" ]]; then
  echo "missing prebuilt experimental RBF: $LOCAL_RBF" >&2
  echo "build it with GitHub Actions: gh workflow run fpga-vblank-latch.yml --repo NigelBreslaw/MiSTer-MagiK --ref main" >&2
  exit 1
fi
test -f "$LOCAL_RBF"
echo "==> Reusing prebuilt experimental RBF: $LOCAL_RBF"

if [[ ! -f "$LOCAL_KO" ]]; then
  "$ROOT/scripts/build-plugin-probe-module.sh"
else
  echo "==> Reusing existing plugin module: $LOCAL_KO"
fi

echo "==> Building Rust diagnostics binary"
"$ROOT/magik-gui/build-arm.sh" --diagnostics --bench-tools

test -f "$LOCAL_KO"
if ! grep -q 'vermagic:.*5\.15\.1-MiSTer' "$ROOT/build/plugin-probe/modinfo.txt"; then
  echo "module vermagic does not target 5.15.1-MiSTer:" >&2
  cat "$ROOT/build/plugin-probe/modinfo.txt" >&2
  exit 1
fi

echo "==> Verifying stock runtime before one-shot experiment"
"$MISTER" status
"$MISTER" run "set -e; uname -r; test \"\$(uname -r)\" = '5.15.1-MiSTer'; command -v insmod; command -v rmmod; test ! -e /dev/mister-magik-scanout-slots || rmmod mister_magik_scanout_slots"

echo "==> Uploading plugin module and experimental RBF"
"$MISTER" run "mkdir -p /media/fat/mister-magik/experiments '$REMOTE_DIR'"
"$MISTER" put "$LOCAL_KO" "$REMOTE_KO"
"$MISTER" put "$LOCAL_RBF" "$REMOTE_RBF"
"$MISTER" run "chmod 600 '$REMOTE_KO'; sync"

echo "==> Loading experimental Menu core through Main MagiK launch path"
"$MISTER" run "printf 'mister_magik_launch $REMOTE_RBF\n' > /dev/MiSTer_cmd; sleep 4"

echo "==> Verifying experimental RBF is the active Main core"
"$MISTER" run "set -e; pid=\$(pidof MiSTer_MagiK); tr '\000' ' ' < /proc/\$pid/cmdline; echo; tr '\000' ' ' < /proc/\$pid/cmdline | grep -F '$REMOTE_RBF'" \
  | tee "$LOCAL_DIR/${LABEL}-active-main-cmdline.log"

echo "==> Uploading diagnostics binary"
"$MISTER" run "pidof mister-magik-fb 2>/dev/null | xargs -r kill -9"
"$MISTER" put "$ROOT/magik-gui/target/armv7-unknown-linux-gnueabihf/release-device/mister-magik-fb" "$REMOTE_BIN"
"$MISTER" run "chmod +x '$REMOTE_BIN'; sync"

echo "==> Loading plugin module"
"$MISTER" run "insmod '$REMOTE_KO'; test -e /dev/mister-magik-scanout-slots; grep '^mister_magik_scanout_slots ' /proc/modules"

echo "==> Restarting Main-supervised launcher with FPGA latch backend"
MISTER_PRESENT_BACKEND=fpga-vblank-latch-hidden MISTER_LAUNCHER_START_SCREEN=home \
  "$ROOT/scripts/run-rust.sh" launcher 0
"$MISTER" run "set -e; for i in \$(seq 1 30); do pidof mister-magik-fb >/dev/null 2>&1 && break; sleep 0.5; done; test -s /tmp/mister-magik/status.json && sed -n '1,80p' /tmp/mister-magik/status.json || true"

echo "==> Running FPGA latch capability report"
"$MISTER" run "'$REMOTE_BIN' fpga-latch-report" | tee "$LOCAL_DIR/${LABEL}-fpga-latch-report.log"
if ! grep -q $'\tsupported=1\t' "$LOCAL_DIR/${LABEL}-fpga-latch-report.log"; then
  echo "FPGA latch commands are not supported after activation; collecting evidence and stopping before capture." >&2
  "$MISTER" run "pid=\$(pidof MiSTer_MagiK 2>/dev/null || true); if [ -n \"\$pid\" ]; then tr '\000' ' ' < /proc/\$pid/cmdline; echo; fi" > "$LOCAL_DIR/${LABEL}-failure-main-cmdline.log" 2>/dev/null || true
  "$MISTER" get /tmp/mister-magik-slint.log "$LOCAL_DIR/${LABEL}-mister-magik-slint.log" >/dev/null 2>&1 || true
  "$MISTER" get /tmp/mister-magik/status.json "$LOCAL_DIR/${LABEL}-status.json" >/dev/null 2>&1 || true
  exit 1
fi

echo "==> Running single-post latch report"
"$MISTER" run "'$REMOTE_BIN' fpga-latch-post-report" | tee "$LOCAL_DIR/${LABEL}-fpga-latch-post-report.log"

echo "==> Running visible latch pattern ($PATTERN_FRAMES frames)"
"$MISTER" run "'$REMOTE_BIN' fpga-latch-pattern '$PATTERN_FRAMES'" | tee "$LOCAL_DIR/${LABEL}-fpga-latch-pattern.log"

if [[ "$CAPTURE" == "1" ]]; then
  echo "==> Capturing real launcher Home pan with FPGA latch backend"
  "$ROOT/scripts/capture-launcher-home-pan-video.sh" "$LABEL" \
    --secs "$SCROLL_SECS" \
    --capture-secs "$((SCROLL_SECS + 10))" \
    --strip-start 15 \
    --fps 25 \
    --present-backend fpga-vblank-latch-hidden
else
  echo "==> Profiling real launcher scroll with FPGA latch backend"
  MISTER_PRESENT_BACKEND=fpga-vblank-latch-hidden \
    "$ROOT/scripts/profile-arcade-scroll.sh" "$LABEL" --skip-build --secs "$SCROLL_SECS" --scenario turbo-hold --skip-boot-prelude --catalog-refresh off --stream-consumer none
fi

echo "==> Restoring normal stock runtime"
restore_normal
trap - EXIT

echo "==> Wrote:"
echo "    $LOCAL_DIR/${LABEL}-active-main-cmdline.log"
echo "    $LOCAL_DIR/${LABEL}-fpga-latch-report.log"
echo "    $LOCAL_DIR/${LABEL}-fpga-latch-post-report.log"
echo "    $LOCAL_DIR/${LABEL}-fpga-latch-pattern.log"
echo "    $ROOT/build/arcade-scroll-profiles/${LABEL}-arcade-scroll.tsv"
if [[ "$CAPTURE" == "1" ]]; then
  echo "    $ROOT/build/launcher-home-pan-captures/${LABEL}.mov"
  echo "    $ROOT/build/launcher-home-pan-captures/${LABEL}.tear-strip.png"
fi
