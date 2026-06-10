#!/bin/bash
# Cross-compile the native MiSTer frontend for armv7 (the DE10-Nano's ARM core).
#
# Profiles (see magik-gui/BUILD.md):
#   ./build-arm.sh              → release (thin LTO + Cortex-A9, launcher UI scope)
#   ./build-arm.sh --fast-dev   → release-fast-dev (no LTO, incremental, launcher UI scope)
#   ./build-arm.sh --opt2       → release-opt2 (experiment: opt-level=2)
#   ./build-arm.sh --opts       → release-opts (experiment: opt-level=s)
#   ./build-arm.sh --incr       → release-incr (experiment: incremental release)
#   ./build-arm.sh --device     → release-device (fat LTO + Cortex-A9, ship to MiSTer)
#   ./build-arm.sh --fast       → alias for release
#   ./build-arm.sh --all-scenes → release with every Slint bench scene
#
# Wraps `cross` with the settings the toolchain needs on an Apple-Silicon host
# (see AGENTS.md §12).
#
# One-time host setup:
#   cargo install cross --locked
#   rustup toolchain add stable-x86_64-unknown-linux-gnu --profile minimal --force-non-host
#
set -euo pipefail
cd "$(dirname "$0")"

PROFILE=release
FEATURES=(ui)
FEATURE_LIST=""
UI_SCOPE="${MISTER_UI_BUILD_SCOPE:-}"
add_feature() {
  local feature="$1"
  local existing
  for existing in "${FEATURES[@]}"; do
    if [ "$existing" = "$feature" ]; then
      return
    fi
  done
  FEATURES+=("$feature")
}
ARGS=("$@")
for ((i = 0; i < ${#ARGS[@]}; i++)); do
  arg="${ARGS[$i]}"
  case "$arg" in
    --device|--release-device) PROFILE=release-device ;;
    --fast-dev|--release-fast-dev) PROFILE=release-fast-dev ;;
    --opt2|--release-opt2) PROFILE=release-opt2 ;;
    --opts|--release-opts) PROFILE=release-opts ;;
    --incr|--release-incr) PROFILE=release-incr ;;
    --profile)
      PROFILE=release-device-profile
      add_feature profile
      ;;
    --video) add_feature video ;;
    --fast|--release) PROFILE=release ;;
    --all-scenes) UI_SCOPE=all; add_feature bench-scenes ;;
    --ui-scope=*) UI_SCOPE="${arg#--ui-scope=}" ;;
    --ui-scope)
      i=$((i + 1))
      if [ "$i" -ge "${#ARGS[@]}" ]; then
        echo "ERROR: --ui-scope requires one of: launcher, arcade, all" >&2
        exit 2
      fi
      UI_SCOPE="${ARGS[$i]}"
      ;;
    -h|--help)
      sed -n '4,12p' ./build-arm.sh | sed 's/^# \{0,1\}//'
      echo "  ./build-arm.sh --video       → include FFmpeg-backed video benchmark"
      echo "  ./build-arm.sh --ui-scope S  → launcher | arcade | all"
      exit 0
      ;;
  esac
done

export DOCKER_DEFAULT_PLATFORM=linux/amd64
export SLINT_FONT_SIZES="${SLINT_FONT_SIZES:-8,16,24,32}"
export RUSTC_WRAPPER=""

if [ -z "$UI_SCOPE" ]; then
  case "$PROFILE" in
    release|release-fast-dev|release-opt2|release-opts|release-incr)
      if [[ " ${FEATURES[*]-} " != *" video "* ]]; then
        UI_SCOPE=launcher
      else
        UI_SCOPE=all
      fi
      ;;
    *) UI_SCOPE=all ;;
  esac
fi
case "$UI_SCOPE" in
  launcher|arcade|all) ;;
  *)
    echo "ERROR: invalid UI scope: $UI_SCOPE (expected launcher, arcade, or all)" >&2
    exit 2
    ;;
esac
if [[ " ${FEATURES[*]-} " == *" video "* ]] && [ "$UI_SCOPE" != all ]; then
  echo "ERROR: --video requires UI scope 'all' because video_playback.slint is a bench scene" >&2
  exit 2
fi
export MISTER_UI_BUILD_SCOPE="$UI_SCOPE"

if ! command -v docker >/dev/null 2>&1; then
  echo "ERROR: Docker is not installed or not on PATH — cross needs it for armv7 builds." >&2
  exit 1
fi

DOCKER_INFO_ERR="$(mktemp)"
if ! docker info >/dev/null 2>"$DOCKER_INFO_ERR"; then
  echo "ERROR: Docker is not reachable — cross needs it for armv7 builds." >&2
  sed -n '1,6p' "$DOCKER_INFO_ERR" >&2
  rm -f "$DOCKER_INFO_ERR"
  exit 1
fi
rm -f "$DOCKER_INFO_ERR"

export RUSTFLAGS="${RUSTFLAGS:+$RUSTFLAGS }-C target-cpu=cortex-a9 -C target-feature=+neon"
if [ "$PROFILE" = release-device-profile ]; then
  export RUSTFLAGS="$RUSTFLAGS -C force-frame-pointers=yes"
  echo "==> cross build profile=release-device-profile ui_scope=$UI_SCOPE (symbols + pprof + Cortex-A9 + NEON target)"
elif [ "$PROFILE" = release-device ]; then
  echo "==> cross build profile=release-device ui_scope=$UI_SCOPE (fat LTO + Cortex-A9 + NEON target)"
elif [ "$PROFILE" = release-fast-dev ]; then
  echo "==> cross build profile=release-fast-dev ui_scope=$UI_SCOPE (no LTO + incremental + Cortex-A9 + NEON target)"
elif [ "$PROFILE" = release-opt2 ]; then
  echo "==> cross build profile=release-opt2 ui_scope=$UI_SCOPE (opt-level=2 + thin LTO + Cortex-A9 + NEON target)"
elif [ "$PROFILE" = release-opts ]; then
  echo "==> cross build profile=release-opts ui_scope=$UI_SCOPE (opt-level=s + thin LTO + Cortex-A9 + NEON target)"
elif [ "$PROFILE" = release-incr ]; then
  echo "==> cross build profile=release-incr ui_scope=$UI_SCOPE (thin LTO + incremental + Cortex-A9 + NEON target)"
else
  echo "==> cross build profile=release ui_scope=$UI_SCOPE (thin LTO + Cortex-A9 + NEON target)"
fi

BUILD_LOG="$(mktemp)"
trap 'rm -f "$BUILD_LOG"' EXIT
BUILD_ARGS=(--target armv7-unknown-linux-gnueabihf --profile "$PROFILE")
if [ "${#FEATURES[@]}" -gt 0 ]; then
  FEATURE_LIST="$(IFS=,; echo "${FEATURES[*]}")"
  BUILD_ARGS+=(--features "$FEATURE_LIST")
fi

if [[ " ${FEATURES[*]-} " == *" video "* ]]; then
  "$PWD/scripts/build-minimal-ffmpeg.sh"
  export FFMPEG_DIR="/project/target/ffmpeg-minimal/armv7/dist"
  export PKG_CONFIG_PATH="/project/target/ffmpeg-minimal/armv7/dist/lib/pkgconfig"
  export PKG_CONFIG_ALLOW_CROSS=1
  echo "==> using minimal FFmpeg: $FFMPEG_DIR"
fi

if ! cross build "${BUILD_ARGS[@]}" 2>&1 | tee "$BUILD_LOG"; then
  exit 1
fi
if grep -q 'Falling back to `cargo` on the host' "$BUILD_LOG"; then
  echo "ERROR: cross fell back to host cargo (Docker not used). Check Docker and run from magik-gui/." >&2
  exit 1
fi
echo "==> cross build OK"

BIN="$PWD/target/armv7-unknown-linux-gnueabihf/$PROFILE/mister-magik-fb"
"$PWD/scripts/record-binary-size.sh" "$PROFILE" "${FEATURE_LIST:-none}" "$BIN"
