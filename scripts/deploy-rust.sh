#!/usr/bin/env bash
# Cross-build the Rust frontend and deploy the binary to the MiSTer.
#
# This is a production-safe file deploy: when Main_MiSTer supervises the
# launcher, deploy asks it to suspend MagiK, swaps the binary, then resumes.
#
#   MISTER_IP=192.168.1.117 MISTER_PASS=1 scripts/deploy-rust.sh
#   MISTER_IP=... scripts/deploy-rust.sh --opt2
#   MISTER_IP=... scripts/deploy-rust.sh --opts
#   MISTER_IP=... scripts/deploy-rust.sh --incr
#   MISTER_IP=... scripts/deploy-rust.sh --all-scenes
#   MISTER_IP=... scripts/deploy-rust.sh --ui-scope launcher
#   MISTER_IP=... scripts/deploy-rust.sh --video
#   MISTER_IP=... scripts/deploy-rust.sh --mame-metadata --asset-packs
#   MISTER_MAME_SOFTWARE_DIR=/path/to/mame/hash scripts/deploy-rust.sh --mame-metadata
#   MISTER_HBMAME_BIN=/path/to/hbmame scripts/deploy-rust.sh --hbmame-metadata
#
# Default installs the release-device (A3) binary.
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
REMOTE_DIR="/media/fat/mister-magik"
REMOTE="$REMOTE_DIR/mister-magik-fb"
REMOTE_ASSET_DIR="$REMOTE_DIR/assets"
DEPLOY_LOCK="$REMOTE_DIR/deploy.lock"
MAME_SQLITE="${MISTER_MAME_SQLITE:-$HERE/build/mame.sqlite3}"
HBMAME_SQLITE="${MISTER_HBMAME_SQLITE:-$HERE/build/hbmame.sqlite3}"
HBMAME_BIN="${MISTER_HBMAME_BIN:-}"
MAME_SOFTWARE_DIR="${MISTER_MAME_SOFTWARE_DIR:-${MISTER_MAME_HASH_DIR:-}}"
NEOGEO_SCREENSHOT_PACK="${MISTER_NEOGEO_SCREENSHOT_PACK:-$HERE/build/neogeo-screenshots/neogeo-screenshots.mmlz4b}"
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
DEPLOY_MAME_METADATA=0
DEPLOY_HBMAME_METADATA=0
DEPLOY_HBMAME_FROM_LIBRARY=0
DEPLOY_ASSET_PACKS=0
ARGS=("$@")
for ((i = 0; i < ${#ARGS[@]}; i++)); do
  arg="${ARGS[$i]}"
  case "$arg" in
    --opt2) PROFILE=release-opt2; BUILD_FLAG=(--opt2) ;;
    --opts) PROFILE=release-opts; BUILD_FLAG=(--opts) ;;
    --incr) PROFILE=release-incr; BUILD_FLAG=(--incr) ;;
    --device) PROFILE=release-device; BUILD_FLAG=(--device) ;;
    --video) DEPLOY_VIDEO=1; BUILD_FLAG+=(--video) ;;
    --mame-metadata) DEPLOY_MAME_METADATA=1 ;;
    --hbmame-metadata) DEPLOY_HBMAME_METADATA=1 ;;
    --asset-packs) DEPLOY_ASSET_PACKS=1 ;;
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
      sed -n '2,10p' "$0" | sed 's/^# \{0,1\}//'
      exit 0
      ;;
    *)
      echo "ERROR: unknown argument: $arg" >&2
      exit 2
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
remote_run() {
  MISTER_IP="${MISTER_IP:-192.168.1.117}" \
  MISTER_PASS="${MISTER_PASS:-1}" \
    "$HERE/scripts/mister" run "$1"
}

magik_command() {
  remote_run "if [ -p /dev/MiSTer_cmd ] && pidof MiSTer_MagiK >/dev/null 2>&1; then printf '$1\n' > /dev/MiSTer_cmd; fi" >/dev/null 2>&1 || true
}

cleanup_deploy_lock() {
  remote_run "rm -f '$DEPLOY_LOCK'" >/dev/null 2>&1 || true
  magik_command "mister_magik_resume"
}
trap cleanup_deploy_lock EXIT
remote_run "mkdir -p '$REMOTE_DIR'; : > '$DEPLOY_LOCK'"
magik_command "mister_magik_suspend"
MISTER_IP="${MISTER_IP:-192.168.1.117}" \
MISTER_PASS="${MISTER_PASS:-1}" \
  "$HERE/scripts/mister" put "$BIN" "$REMOTE.upload"
remote_run "mv '$REMOTE.upload' '$REMOTE'; chmod +x '$REMOTE'; rm -f '$DEPLOY_LOCK'"
magik_command "mister_magik_resume"
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
if [ "$DEPLOY_MAME_METADATA" -eq 1 ]; then
  BUILD_MAME_METADATA=0
  if [ ! -f "$MAME_SQLITE" ]; then
    BUILD_MAME_METADATA=1
  elif [ -n "$MAME_SOFTWARE_DIR" ]; then
    BUILD_MAME_METADATA=1
  fi
  if [ "$BUILD_MAME_METADATA" -eq 1 ]; then
    echo "==> Building MAME metadata DB at $MAME_SQLITE"
    mkdir -p "$(dirname "$MAME_SQLITE")"
    mame_metadata_args=(mame-metadata-build --out "$MAME_SQLITE")
    if [ -n "$MAME_SOFTWARE_DIR" ] && [ -f "$MAME_SQLITE" ]; then
      mame_metadata_args+=(--machine-sqlite "$MAME_SQLITE")
    fi
    if [ -n "$MAME_SOFTWARE_DIR" ]; then
      mame_metadata_args+=(--software-dir "$MAME_SOFTWARE_DIR")
    fi
    "$HERE/scripts/mister" "${mame_metadata_args[@]}"
  fi
  echo "==> Deploying $MAME_SQLITE -> $REMOTE_DIR/mame.sqlite3"
  MISTER_IP="${MISTER_IP:-192.168.1.117}" \
  MISTER_PASS="${MISTER_PASS:-1}" \
    "$HERE/scripts/mister" put "$MAME_SQLITE" "$REMOTE_DIR/mame.sqlite3.upload"
  remote_run "mv '$REMOTE_DIR/mame.sqlite3.upload' '$REMOTE_DIR/mame.sqlite3'"
fi
if [ "$DEPLOY_HBMAME_METADATA" -eq 1 ]; then
  if [ ! -f "$HBMAME_SQLITE" ]; then
    if [ -z "$HBMAME_BIN" ]; then
      DEPLOY_HBMAME_FROM_LIBRARY=1
      echo "==> No local HBMame metadata DB; will build supplemental metadata from device library"
    else
      echo "==> Building HBMame metadata DB at $HBMAME_SQLITE"
      mkdir -p "$(dirname "$HBMAME_SQLITE")"
      "$HERE/scripts/mister" mame-metadata-build --out "$HBMAME_SQLITE" --mame "$HBMAME_BIN"
    fi
  fi
  if [ "$DEPLOY_HBMAME_FROM_LIBRARY" -eq 0 ]; then
    echo "==> Deploying $HBMAME_SQLITE -> $REMOTE_DIR/hbmame.sqlite3"
    MISTER_IP="${MISTER_IP:-192.168.1.117}" \
    MISTER_PASS="${MISTER_PASS:-1}" \
      "$HERE/scripts/mister" put "$HBMAME_SQLITE" "$REMOTE_DIR/hbmame.sqlite3.upload"
    remote_run "mv '$REMOTE_DIR/hbmame.sqlite3.upload' '$REMOTE_DIR/hbmame.sqlite3'"
  fi
fi
if [ "$DEPLOY_ASSET_PACKS" -eq 1 ]; then
  if [ ! -f "$NEOGEO_SCREENSHOT_PACK" ]; then
    echo "ERROR: --asset-packs requested but $NEOGEO_SCREENSHOT_PACK does not exist" >&2
    echo "       Build it with: scripts/build-neogeo-screenshot-pack.sh" >&2
    exit 1
  fi
  echo "==> Deploying $NEOGEO_SCREENSHOT_PACK -> $REMOTE_ASSET_DIR/neogeo-screenshots.mmlz4b"
  remote_run "mkdir -p '$REMOTE_ASSET_DIR'"
  MISTER_IP="${MISTER_IP:-192.168.1.117}" \
  MISTER_PASS="${MISTER_PASS:-1}" \
    "$HERE/scripts/mister" put "$NEOGEO_SCREENSHOT_PACK" "$REMOTE_ASSET_DIR/neogeo-screenshots.mmlz4b.upload"
  remote_run "mv '$REMOTE_ASSET_DIR/neogeo-screenshots.mmlz4b.upload' '$REMOTE_ASSET_DIR/neogeo-screenshots.mmlz4b'"
fi
if [ "$DEPLOY_HBMAME_FROM_LIBRARY" -eq 1 ]; then
  echo "==> Building supplemental HBMame metadata from device library"
  if ! remote_run "$REMOTE hbmame-metadata-from-library"; then
    echo "==> Device library unavailable; refreshing once before supplemental metadata"
    remote_run "$REMOTE library-refresh"
    remote_run "$REMOTE hbmame-metadata-from-library"
  fi
  echo "==> Refreshing library DB on device with supplemental HBMame metadata"
  remote_run "$REMOTE library-refresh"
elif [ "$DEPLOY_MAME_METADATA" -eq 1 ] || [ "$DEPLOY_HBMAME_METADATA" -eq 1 ] || [ "$DEPLOY_ASSET_PACKS" -eq 1 ]; then
  echo "==> Refreshing library DB on device"
  remote_run "$REMOTE library-refresh"
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
echo "    Main-supervised launcher was suspended and resumed when available."
echo "    Production boot: scripts/install-slint-boot.sh  (once — MiSTer.ini main= handoff)"
echo "    Restart only:    scripts/run-rust.sh launcher 0  (no build, no copy)"
echo "    Arcade bench:    scripts/profile-preview-scroll.sh 30 held-scroll LABEL"
echo "    Diagnostics:     scripts/mister-asset-diagnostics.sh"
echo "    Restore stock:   scripts/restore-stock-boot.sh"
