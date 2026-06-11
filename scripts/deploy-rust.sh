#!/usr/bin/env bash
# Cross-build the Rust frontend and deploy the binary to the MiSTer.
#
#   MISTER_IP=192.168.1.117 MISTER_PASS=1 scripts/deploy-rust.sh
#   MISTER_IP=... scripts/deploy-rust.sh --fast    # thin LTO, quicker build
#   MISTER_IP=... scripts/deploy-rust.sh --fast-dev
#   MISTER_IP=... scripts/deploy-rust.sh --opt2
#   MISTER_IP=... scripts/deploy-rust.sh --opts
#   MISTER_IP=... scripts/deploy-rust.sh --incr
#   MISTER_IP=... scripts/deploy-rust.sh --fast --all-scenes
#   MISTER_IP=... scripts/deploy-rust.sh --fast --ui-scope arcade
#   MISTER_IP=... scripts/deploy-rust.sh --fast --video
#
# Default installs the release-device (A3) binary — use --fast for daily iteration.
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
REMOTE_DIR="/media/fat/mister-magik"
REMOTE="$REMOTE_DIR/mister-magik-fb"
DEPLOY_LOCK="$REMOTE_DIR/deploy.lock"
DEFAULT_VIDEO_SRC="$HERE/build/video/mslug3_320x224_60_h264_baseline_pcm_s16le_mono.mov"
if [ ! -f "$DEFAULT_VIDEO_SRC" ]; then
  DEFAULT_VIDEO_SRC="$HERE/build/video/mslug3_320x224_60_h264_baseline_pcm_s16le.mov"
fi
if [ ! -f "$DEFAULT_VIDEO_SRC" ]; then
  DEFAULT_VIDEO_SRC="$HERE/build/video/mslug3_320x224_60_h264_baseline_crf28.mp4"
fi
if [ ! -f "$DEFAULT_VIDEO_SRC" ]; then
  DEFAULT_VIDEO_SRC="/Users/nigelb/Desktop/mslug3.mp4"
fi
VIDEO_SRC="${MISTER_VIDEO_SRC:-$DEFAULT_VIDEO_SRC}"
VIDEO_REMOTE="/media/fat/mister-magik/mslug3.mov"

PROFILE=release-device
BUILD_FLAG=(--device)
DEPLOY_VIDEO=0
ARGS=("$@")
for ((i = 0; i < ${#ARGS[@]}; i++)); do
  arg="${ARGS[$i]}"
  case "$arg" in
    --fast) PROFILE=release; BUILD_FLAG=(--fast) ;;
    --fast-dev) PROFILE=release-fast-dev; BUILD_FLAG=(--fast-dev) ;;
    --opt2) PROFILE=release-opt2; BUILD_FLAG=(--opt2) ;;
    --opts) PROFILE=release-opts; BUILD_FLAG=(--opts) ;;
    --incr) PROFILE=release-incr; BUILD_FLAG=(--incr) ;;
    --device) PROFILE=release-device; BUILD_FLAG=(--device) ;;
    --video) DEPLOY_VIDEO=1; BUILD_FLAG+=(--video) ;;
    --all-scenes) BUILD_FLAG+=(--all-scenes) ;;
    --ui-scope=*) BUILD_FLAG+=("$arg") ;;
    --ui-scope)
      i=$((i + 1))
      if [ "$i" -ge "${#ARGS[@]}" ]; then
        echo "ERROR: --ui-scope requires one of: launcher, arcade, all" >&2
        exit 2
      fi
      BUILD_FLAG+=(--ui-scope "${ARGS[$i]}")
      ;;
    -h|--help)
      sed -n '2,8p' "$0" | sed 's/^# \{0,1\}//'
      exit 0
      ;;
  esac
done

BIN="$HERE/magik-gui/target/armv7-unknown-linux-gnueabihf/$PROFILE/mister-magik-fb"

bytes() {
  stat -f%z "$1" 2>/dev/null || stat -c%s "$1"
}

human_bytes() {
  awk -v b="$1" 'BEGIN {
    split("B KiB MiB GiB", u, " ");
    n = b + 0;
    i = 1;
    while (n >= 1024 && i < 4) { n /= 1024; i++ }
    if (i == 1) printf "%d %s", n, u[i];
    else printf "%.2f %s", n, u[i];
  }'
}

echo "==> Cross-building (armv7 profile=$PROFILE)"
"$HERE/magik-gui/build-arm.sh" "${BUILD_FLAG[@]}"

LOCAL_BYTES="$(bytes "$BIN")"
echo "==> Local binary size: $LOCAL_BYTES bytes ($(human_bytes "$LOCAL_BYTES"))"

echo "==> Deploying $BIN -> $REMOTE"
cleanup_deploy_lock() {
  MISTER_IP="${MISTER_IP:-192.168.1.117}" \
  MISTER_PASS="${MISTER_PASS:-1}" \
    "$HERE/scripts/mister" run "rm -f '$DEPLOY_LOCK'" >/dev/null 2>&1 || true
}
trap cleanup_deploy_lock EXIT
MISTER_IP="${MISTER_IP:-192.168.1.117}" \
MISTER_PASS="${MISTER_PASS:-1}" \
  "$HERE/scripts/mister" run "mkdir -p '$REMOTE_DIR'; : > '$DEPLOY_LOCK'; kill -9 \$(pidof mister-magik-fb) 2>/dev/null || true"
MISTER_IP="${MISTER_IP:-192.168.1.117}" \
MISTER_PASS="${MISTER_PASS:-1}" \
  "$HERE/scripts/mister" put "$BIN" "$REMOTE.upload"
MISTER_IP="${MISTER_IP:-192.168.1.117}" \
MISTER_PASS="${MISTER_PASS:-1}" \
  "$HERE/scripts/mister" run "mv '$REMOTE.upload' '$REMOTE'; chmod +x '$REMOTE'; rm -f '$DEPLOY_LOCK'"
trap - EXIT
if [ "$DEPLOY_VIDEO" -eq 1 ]; then
  if [ ! -f "$VIDEO_SRC" ]; then
    echo "ERROR: --video requested but $VIDEO_SRC does not exist" >&2
    exit 1
  fi
  echo "==> Deploying $VIDEO_SRC -> $VIDEO_REMOTE"
  MISTER_IP="${MISTER_IP:-192.168.1.117}" \
  MISTER_PASS="${MISTER_PASS:-1}" \
    "$HERE/scripts/mister" put "$VIDEO_SRC" "$VIDEO_REMOTE"
fi
MISTER_IP="${MISTER_IP:-192.168.1.117}" \
MISTER_PASS="${MISTER_PASS:-1}" \
  "$HERE/scripts/mister" run "chmod +x $REMOTE"

REMOTE_BYTES="$(
  MISTER_IP="${MISTER_IP:-192.168.1.117}" \
  MISTER_PASS="${MISTER_PASS:-1}" \
    "$HERE/scripts/mister" run "wc -c $REMOTE" \
    | awk '{print $1}' | tail -1
)"
if [ -n "$REMOTE_BYTES" ]; then
  echo "==> Deployed binary size: $REMOTE_BYTES bytes ($(human_bytes "$REMOTE_BYTES"))"
fi

echo "==> Deployed ($PROFILE)."
echo "    Production boot: scripts/install-slint-boot.sh  (once — MiSTer.ini main= handoff)"
echo "    Restart only:    scripts/run-rust.sh launcher 0  (no build, no copy)"
echo "    Dev / bench:     scripts/run-rust.sh arcade 0"
echo "    Restore stock:   scripts/restore-stock-boot.sh"
