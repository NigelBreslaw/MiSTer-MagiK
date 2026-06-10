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
CARGO_CACHE="${MISTER_APPLE_CONTAINER_CARGO_HOME:-$HOME/.cargo}"
RUST_TOOLCHAIN="${MISTER_ARM64_RUST_TOOLCHAIN:-$HOME/.rustup/toolchains/stable-aarch64-unknown-linux-gnu}"
CONTAINER_CPUS="${MISTER_APPLE_CONTAINER_CPUS:-3}"
CONTAINER_MEMORY="${MISTER_APPLE_CONTAINER_MEMORY:-5g}"
TARGET=armv7-unknown-linux-gnueabihf

PROFILE=release
FEATURES=(ui)
FEATURE_LIST=""
UI_SCOPE="${MISTER_UI_BUILD_SCOPE:-}"
LOCKED=1

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
  ./build-arm64-apple-container.sh              → release (thin LTO, launcher UI scope)
  ./build-arm64-apple-container.sh --fast-dev   → release-fast-dev
  ./build-arm64-apple-container.sh --opt2       → release-opt2
  ./build-arm64-apple-container.sh --opts       → release-opts
  ./build-arm64-apple-container.sh --incr       → release-incr
  ./build-arm64-apple-container.sh --device     → release-device
  ./build-arm64-apple-container.sh --all-scenes → compile every Slint bench scene
  ./build-arm64-apple-container.sh --ui-scope S → launcher | arcade | all

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
    --fast-dev|--release-fast-dev) PROFILE=release-fast-dev ;;
    --opt2|--release-opt2) PROFILE=release-opt2 ;;
    --opts|--release-opts) PROFILE=release-opts ;;
    --incr|--release-incr) PROFILE=release-incr ;;
    --profile)
      PROFILE=release-device-profile
      add_feature profile
      ;;
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
    --video)
      echo "ERROR: --video is not supported by build-arm64-apple-container.sh yet; use build-arm.sh --video." >&2
      exit 2
      ;;
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
    release|release-fast-dev|release-opt2|release-opts|release-incr) UI_SCOPE=launcher ;;
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

mkdir -p "$TARGET_DIR" "$CARGO_CACHE"

echo "==> host arch: $(uname -m)"
echo "==> container tool: $(container --version 2>&1 | head -n 1)"
echo "==> rust toolchain: $RUST_TOOLCHAIN"
echo "==> target triple: $TARGET"
echo "==> building linux/arm64 cross image: $IMAGE"
container build --arch arm64 --file Dockerfile.cross-armv7 --tag "$IMAGE" .

FEATURE_LIST="$(IFS=,; echo "${FEATURES[*]}")"
BUILD_ARGS=(build --target "$TARGET" --profile "$PROFILE" --features "$FEATURE_LIST")
if [ "$LOCKED" -eq 1 ]; then
  BUILD_ARGS+=(--locked)
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
  --volume "$CARGO_CACHE:/cargo" \
  --volume "$RUST_TOOLCHAIN:/rust:ro" \
  --volume "$PWD:/project" \
  --volume "$TARGET_DIR:/target" \
  --workdir /project \
  "$IMAGE" \
  sh -lc 'PATH=/rust/bin:$PATH cargo "$@"' sh "${BUILD_ARGS[@]}"

BIN="$TARGET_DIR/$TARGET/$PROFILE/mister-magik-fb"
if [ ! -f "$BIN" ]; then
  echo "ERROR: expected binary not found: $BIN" >&2
  exit 1
fi

BYTES="$(stat -f%z "$BIN" 2>/dev/null || stat -c%s "$BIN")"
echo "==> build OK: $BIN"
echo "==> binary size: $BYTES bytes"
