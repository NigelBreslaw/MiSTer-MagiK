#!/bin/bash
# Cross-compile the native MiSTer frontend for armv7 (the DE10-Nano's ARM core).
#
# Profiles (see magik-gui/BUILD.md):
#   ./build-arm.sh              → release-device (fat LTO + Cortex-A9, ship to MiSTer)
#   ./build-arm.sh --device     → release-device (fat LTO + Cortex-A9, ship to MiSTer)
#   ./build-arm.sh --all-scenes → release-device with bench scenes + experiments
#   ./build-arm.sh --experiments → release-device with experimental effect scenes
#   ./build-arm.sh --video      → release-device with production fast-path video
#   ./build-arm.sh --video-lab  → release-device with video comparison/fallback paths
#   ./build-arm.sh --diagnostics → release-device with diagnostics commands
#   ./build-arm.sh --bench-tools → release-device with device benchmark commands
#
# Every build emits a Cargo timing report under target/cargo-timings/ so we can
# spot expensive crates and accidental target/feature creep.
#
# Uses the Apple Virtualization Framework container backend on Apple Silicon,
# and falls back to cross/Docker on Linux and CI (see magik-gui/BUILD.md).
#
# One-time host setup:
#   rustup toolchain add stable-aarch64-unknown-linux-gnu --profile minimal --force-non-host
#   rustup target add armv7-unknown-linux-gnueabihf --toolchain stable-aarch64-unknown-linux-gnu
#   container system start
#
set -euo pipefail
cd "$(dirname "$0")"

BACKEND="${MISTER_ARM_BUILD_BACKEND:-auto}"
case "$BACKEND" in
  auto|apple-container|cross) ;;
  *)
    echo "ERROR: invalid MISTER_ARM_BUILD_BACKEND=$BACKEND (expected auto, apple-container, or cross)" >&2
    exit 2
    ;;
esac
for arg in "$@"; do
  case "$arg" in
    -h|--help) BACKEND=cross ;;
  esac
done
if [ "$BACKEND" = auto ] || [ "$BACKEND" = apple-container ]; then
  if [ "$(uname -s)" = Darwin ] && [ "$(uname -m)" = arm64 ]; then
    if ! command -v container >/dev/null 2>&1; then
      if [ "$BACKEND" = apple-container ]; then
        echo "ERROR: Apple container is not installed or not on PATH." >&2
        exit 1
      fi
    else
      exec "$PWD/build-arm64-apple-container.sh" "$@"
    fi
  elif [ "$BACKEND" = apple-container ]; then
    echo "ERROR: Apple-container backend requires arm64 macOS; got $(uname -s)/$(uname -m)." >&2
    exit 1
  fi
fi

PROFILE=release-device
FEATURES=(ui)
FEATURE_LIST=""
BIN_TARGET=""
BIN_NAME="mister-magik-fb"
MANIFEST_PATH=""
UI_SCOPE="${MISTER_UI_BUILD_SCOPE:-}"
CLEAN=0
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
    --profile)
      PROFILE=release-device-profile
      add_feature profile
      ;;
    --video) add_feature video ;;
    --video-lab)
      add_feature video
      add_feature video-lab
      ;;
    --diagnostics) add_feature diagnostics ;;
    --bench-tools) add_feature bench-tools ;;
    --catalog-builder)
      FEATURES=(builder)
      BIN_TARGET="mister-magik-catalog-builder"
      BIN_NAME="mister-magik-catalog-builder"
      MANIFEST_PATH="catalog/Cargo.toml"
      UI_SCOPE=all
      ;;
    --clean) CLEAN=1 ;;
    --all-scenes) UI_SCOPE=all; add_feature experiments ;;
    --experiments) UI_SCOPE=all; add_feature experiments ;;
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
      sed -n '4,9p' ./build-arm.sh | sed 's/^# \{0,1\}//'
      echo "  ./build-arm.sh --video       → include FFmpeg-backed video benchmark"
      echo "  ./build-arm.sh --video-lab   → include video comparison/fallback paths"
      echo "  ./build-arm.sh --diagnostics → include diagnostics commands"
      echo "  ./build-arm.sh --bench-tools → include device benchmark commands"
      echo "  ./build-arm.sh --catalog-builder → build only the Slint-free catalog builder"
      echo "  ./build-arm.sh --ui-scope S  → launcher | arcade | all"
      echo "  ./build-arm.sh --clean       → cargo clean before building"
      exit 0
      ;;
    *)
      echo "ERROR: unknown argument: $arg" >&2
      exit 2
      ;;
  esac
done

export DOCKER_DEFAULT_PLATFORM=linux/amd64
export SLINT_FONT_SIZES="${SLINT_FONT_SIZES:-8,16,24,32}"
export RUSTC_WRAPPER=""

if [ -z "$UI_SCOPE" ]; then
  UI_SCOPE=all
fi
case "$UI_SCOPE" in
  launcher|arcade|all) ;;
  *)
    echo "ERROR: invalid UI scope: $UI_SCOPE (expected launcher, arcade, or all)" >&2
    exit 2
    ;;
esac
export MISTER_UI_BUILD_SCOPE="$UI_SCOPE"
export MISTER_MAGIK_BUILD_NUMBER="${MISTER_MAGIK_BUILD_NUMBER:-$(
  git -C "$PWD/.." rev-list --count HEAD 2>/dev/null || echo unknown
)}"
export MISTER_MAGIK_VERSION="${MISTER_MAGIK_VERSION:-0.2.$MISTER_MAGIK_BUILD_NUMBER}"
export MISTER_MAGIK_BUILD_TIME="${MISTER_MAGIK_BUILD_TIME:-$(
  date '+%-d.%-m.%Y %H:%M' 2>/dev/null || date '+%d.%m.%Y %H:%M' 2>/dev/null || echo unknown
)}"

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

export RUSTFLAGS="${RUSTFLAGS:+$RUSTFLAGS }-D warnings -C target-cpu=cortex-a9"
if [ "$PROFILE" = release-device-profile ]; then
  export RUSTFLAGS="$RUSTFLAGS -C force-frame-pointers=yes"
  echo "==> cross build profile=release-device-profile ui_scope=$UI_SCOPE (symbols + pprof + Cortex-A9 target, warnings denied)"
elif [ "$PROFILE" = release-device ]; then
  echo "==> cross build profile=release-device ui_scope=$UI_SCOPE (fat LTO + Cortex-A9 target, warnings denied)"
fi

BUILD_LOG="$(mktemp)"
STAGED_LICENSE="$PWD/LICENSE"
if ! (set -o noclobber; : >"$STAGED_LICENSE") 2>/dev/null; then
  echo "ERROR: refusing to overwrite existing $STAGED_LICENSE" >&2
  rm -f "$BUILD_LOG"
  exit 1
fi
trap 'rm -f "$BUILD_LOG" "$STAGED_LICENSE"' EXIT
cp "$PWD/../LICENSE" "$STAGED_LICENSE"
if [ "$CLEAN" -eq 1 ]; then
  echo "==> cargo clean"
  cargo clean
fi
BUILD_ARGS=(--locked --target armv7-unknown-linux-gnueabihf --profile "$PROFILE")
if [ -n "$MANIFEST_PATH" ]; then
  BUILD_ARGS+=(--manifest-path "$MANIFEST_PATH")
fi
if [ "${MISTER_CARGO_TIMINGS:-1}" != "0" ]; then
  BUILD_ARGS+=(--timings)
fi
if [ -n "$BIN_TARGET" ]; then
  BUILD_ARGS+=(--bin "$BIN_TARGET")
fi
if [ "${#FEATURES[@]}" -gt 0 ]; then
  FEATURE_LIST="$(IFS=,; echo "${FEATURES[*]}")"
  BUILD_ARGS+=(--features "$FEATURE_LIST")
fi

if [[ " ${FEATURES[*]-} " == *" video "* ]]; then
  if [[ " ${FEATURES[*]-} " == *" video-lab "* ]]; then
    MISTER_FFMPEG_VIDEO_LAB=1 "$PWD/scripts/build-minimal-ffmpeg.sh"
  else
    "$PWD/scripts/build-minimal-ffmpeg.sh"
  fi
  export FFMPEG_DIR="/target/ffmpeg-minimal/armv7/dist"
  export PKG_CONFIG_PATH="/target/ffmpeg-minimal/armv7/dist/lib/pkgconfig"
  export PKG_CONFIG_ALLOW_CROSS=1
  export CFLAGS="${CFLAGS:+$CFLAGS }-I/target/ffmpeg-minimal/armv7/dist/include"
  export HOST_CFLAGS="${HOST_CFLAGS:+$HOST_CFLAGS }-I/target/ffmpeg-minimal/armv7/dist/include"
  export CFLAGS_x86_64_unknown_linux_gnu="${CFLAGS_x86_64_unknown_linux_gnu:+$CFLAGS_x86_64_unknown_linux_gnu }-I/target/ffmpeg-minimal/armv7/dist/include"
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
if [ "${MISTER_CARGO_TIMINGS:-1}" != "0" ]; then
  TIMING_REPORT="$(find "$PWD/target/cargo-timings" -type f -name 'cargo-timing*.html' -print 2>/dev/null | sort | tail -1 || true)"
  if [ -n "$TIMING_REPORT" ]; then
    echo "==> Cargo timing report: $TIMING_REPORT"
  fi
fi

BIN="$PWD/target/armv7-unknown-linux-gnueabihf/$PROFILE/$BIN_NAME"
printf '%s\n' "${FEATURE_LIST:-none}" >"$BIN.features"
source "$PWD/../scripts/bench-context-lib.sh"
bench_context_write_build_receipt "$BIN" "$PWD/.." "$PROFILE" "${FEATURE_LIST:-none}" "$UI_SCOPE"
"$PWD/scripts/record-binary-size.sh" "$PROFILE" "${FEATURE_LIST:-none}" "$BIN"
