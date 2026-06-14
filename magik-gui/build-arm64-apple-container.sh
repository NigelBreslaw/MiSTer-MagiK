#!/usr/bin/env bash
# Native Apple-container path for ARMv7 MiSTer builds on Apple Silicon.
#
# This is the GitHub macos-26 / local Apple Silicon counterpart to
# build-arm64-docker.sh. It intentionally does not use cross-rs or Docker's
# linux/amd64 compatibility path.
set -euo pipefail
cd "$(dirname "$0")"

IMAGE="${MISTER_APPLE_CONTAINER_IMAGE:-mister-magik-cross-armv7:ubuntu20-arm64}"
TARGET_DIR="${MISTER_APPLE_CONTAINER_TARGET_DIR:-/private/tmp/mister-magik-apple-container-target}"
MIRROR_TARGET_DIR="${MISTER_APPLE_CONTAINER_MIRROR_TARGET_DIR:-$PWD/target}"
CARGO_CACHE="${MISTER_APPLE_CONTAINER_CARGO_HOME:-$HOME/.cargo}"
RUST_TOOLCHAIN="${MISTER_ARM64_RUST_TOOLCHAIN:-$HOME/.rustup/toolchains/stable-aarch64-unknown-linux-gnu}"
CONTAINER_CPUS="${MISTER_APPLE_CONTAINER_CPUS:-3}"
CONTAINER_MEMORY="${MISTER_APPLE_CONTAINER_MEMORY:-5g}"
TARGET=armv7-unknown-linux-gnueabihf

PROFILE=release-device
FEATURES=(ui)
FEATURE_LIST=""
UI_SCOPE="${MISTER_UI_BUILD_SCOPE:-}"
LOCKED=1
CLEAN=0
BIN_TARGET=""
BIN_NAME="mister-magik-fb"

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

usage() {
  cat <<'EOF'
Native Apple-container ARMv7 build:
  ./build-arm64-apple-container.sh              → release-device
  ./build-arm64-apple-container.sh --opt2       → release-opt2
  ./build-arm64-apple-container.sh --opts       → release-opts
  ./build-arm64-apple-container.sh --incr       → release-incr
  ./build-arm64-apple-container.sh --device     → release-device
  ./build-arm64-apple-container.sh --all-scenes → compile every Slint bench scene
  ./build-arm64-apple-container.sh --video      → include FFmpeg-backed video benchmark
  ./build-arm64-apple-container.sh --ui-scope S → launcher | arcade | all
  ./build-arm64-apple-container.sh --clean      → clear the Apple-container target cache first
  ./build-arm64-apple-container.sh --preview-archive-bench → build only the preview archive benchmark

One-time host setup:
  rustup toolchain add stable-aarch64-unknown-linux-gnu --profile minimal --force-non-host
  rustup target add armv7-unknown-linux-gnueabihf --toolchain stable-aarch64-unknown-linux-gnu
  container system start
  container builder start --cpus 3 --memory 5g
EOF
}

ARGS=("$@")
for ((i = 0; i < ${#ARGS[@]}; i++)); do
  arg="${ARGS[$i]}"
  case "$arg" in
    --device|--release-device) PROFILE=release-device ;;
    --opt2|--release-opt2) PROFILE=release-opt2 ;;
    --opts|--release-opts) PROFILE=release-opts ;;
    --incr|--release-incr) PROFILE=release-incr ;;
    --profile)
      PROFILE=release-device-profile
      add_feature profile
      ;;
    --preview-archive-bench)
      FEATURES=(preview-archive-bench)
      BIN_TARGET=preview-archive-bench
      BIN_NAME=preview-archive-bench
      ;;
    --video) add_feature video ;;
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
    --clean) CLEAN=1 ;;
    --unlocked) LOCKED=0 ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      echo "ERROR: unknown argument: $arg" >&2
      usage >&2
      exit 2
      ;;
  esac
done

if [ -z "$UI_SCOPE" ]; then
  case "$PROFILE" in
    release-opt2|release-opts|release-incr) UI_SCOPE=launcher ;;
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
export MISTER_UI_BUILD_SCOPE="$UI_SCOPE"
if [[ " ${FEATURES[*]-} " == *" video "* ]] && [ "$UI_SCOPE" != all ]; then
  echo "ERROR: --video requires UI scope 'all' because video_playback.slint is a bench scene" >&2
  exit 2
fi

if [ "$(uname -m)" != arm64 ]; then
  echo "ERROR: Apple-container native path requires an arm64 macOS host; got $(uname -m)." >&2
  exit 1
fi
if ! command -v container >/dev/null 2>&1; then
  echo "ERROR: Apple container is not installed or not on PATH." >&2
  exit 1
fi
if [ ! -x "$RUST_TOOLCHAIN/bin/cargo" ]; then
  echo "ERROR: missing linux/aarch64 Rust toolchain at $RUST_TOOLCHAIN" >&2
  echo "Install it with:" >&2
  echo "  rustup toolchain add stable-aarch64-unknown-linux-gnu --profile minimal --force-non-host" >&2
  echo "  rustup target add $TARGET --toolchain stable-aarch64-unknown-linux-gnu" >&2
  exit 1
fi
if [ ! -d "$RUST_TOOLCHAIN/lib/rustlib/$TARGET" ]; then
  echo "ERROR: missing $TARGET std for stable-aarch64-unknown-linux-gnu" >&2
  echo "Install it with:" >&2
  echo "  rustup target add $TARGET --toolchain stable-aarch64-unknown-linux-gnu" >&2
  exit 1
fi

if [ "$CLEAN" -eq 1 ]; then
  echo "==> clearing Apple-container target cache: $TARGET_DIR"
  rm -rf "$TARGET_DIR"
  rm -rf "$MIRROR_TARGET_DIR/$TARGET"
fi
mkdir -p "$TARGET_DIR" "$CARGO_CACHE"

echo "==> host arch: $(uname -m)"
echo "==> container tool: $(container --version 2>&1 | head -n 1)"
echo "==> rust toolchain: $RUST_TOOLCHAIN"
echo "==> target triple: $TARGET"
echo "==> build backend: apple-container"
echo "==> building linux/arm64 cross image: $IMAGE"
container build --arch arm64 --file Dockerfile.cross-armv7 --tag "$IMAGE" .

FEATURE_LIST="$(IFS=,; echo "${FEATURES[*]}")"
BUILD_ARGS=(build --target "$TARGET" --profile "$PROFILE" --features "$FEATURE_LIST")
if [ "$LOCKED" -eq 1 ]; then
  BUILD_ARGS+=(--locked)
fi
if [ -n "$BIN_TARGET" ]; then
  BUILD_ARGS+=(--bin "$BIN_TARGET")
fi

EXTRA_ENVS=()
if [[ " ${FEATURES[*]-} " == *" video "* ]]; then
  "$PWD/scripts/build-minimal-ffmpeg.sh"
  EXTRA_ENVS+=(
    --env FFMPEG_DIR=/project/target/ffmpeg-minimal/armv7/dist
    --env PKG_CONFIG_PATH=/project/target/ffmpeg-minimal/armv7/dist/lib/pkgconfig
    --env PKG_CONFIG_ALLOW_CROSS=1
    --env CFLAGS=-I/project/target/ffmpeg-minimal/armv7/dist/include
    --env HOST_CFLAGS=-I/project/target/ffmpeg-minimal/armv7/dist/include
    --env CFLAGS_aarch64_unknown_linux_gnu=-I/project/target/ffmpeg-minimal/armv7/dist/include
  )
  echo "==> using minimal FFmpeg: /project/target/ffmpeg-minimal/armv7/dist"
fi

HOST_RUSTFLAGS="${RUSTFLAGS:-}"
CONTAINER_RUSTFLAGS="${HOST_RUSTFLAGS:+$HOST_RUSTFLAGS }-C target-cpu=cortex-a9 -C target-feature=+neon"
if [ "$PROFILE" = release-device-profile ]; then
  CONTAINER_RUSTFLAGS="$CONTAINER_RUSTFLAGS -C force-frame-pointers=yes"
fi

echo "==> image arch probe"
container run --arch arm64 --rm "$IMAGE" uname -m
echo "==> container build profile=$PROFILE ui_scope=$UI_SCOPE features=$FEATURE_LIST"
echo "==> target dir: $TARGET_DIR"
container run --arch arm64 --rm \
  --cpus "$CONTAINER_CPUS" \
  --memory "$CONTAINER_MEMORY" \
  --env CARGO_HOME=/cargo \
  --env CARGO_TARGET_DIR=/target \
  --env MISTER_UI_BUILD_SCOPE="$UI_SCOPE" \
  --env RUSTC_WRAPPER= \
  --env RUSTFLAGS="$CONTAINER_RUSTFLAGS" \
  --env SLINT_FONT_SIZES="${SLINT_FONT_SIZES:-8,16,24,32}" \
  "${EXTRA_ENVS[@]}" \
  --volume "$CARGO_CACHE:/cargo" \
  --volume "$RUST_TOOLCHAIN:/rust:ro" \
  --volume "$PWD:/project" \
  --volume "$TARGET_DIR:/target" \
  --workdir /project \
  "$IMAGE" \
  sh -lc 'PATH=/rust/bin:$PATH cargo "$@"' sh "${BUILD_ARGS[@]}"

BIN="$TARGET_DIR/$TARGET/$PROFILE/$BIN_NAME"
if [ ! -f "$BIN" ]; then
  echo "ERROR: expected binary not found: $BIN" >&2
  exit 1
fi

MIRROR_BIN="$MIRROR_TARGET_DIR/$TARGET/$PROFILE/$BIN_NAME"
mkdir -p "$(dirname "$MIRROR_BIN")"
cp "$BIN" "$MIRROR_BIN"

BYTES="$(stat -f%z "$BIN" 2>/dev/null || stat -c%s "$BIN")"
echo "==> build OK: $BIN"
echo "==> mirrored binary: $MIRROR_BIN"
echo "==> binary size: $BYTES bytes"
"$PWD/scripts/record-binary-size.sh" "$PROFILE" "${FEATURE_LIST:-none}" "$MIRROR_BIN"
