#!/usr/bin/env bash
# Native Apple-Silicon Docker path for ARMv7 MiSTer builds.
#
# This intentionally does not use cross-rs. cross 0.2.5 rewrites the Rust
# sysroot to x86_64-unknown-linux-gnu on non-Linux hosts, which breaks when the
# container itself is linux/arm64. This script mounts a linux/aarch64 Rust
# toolchain into a linux/arm64 image and builds the armv7 target directly.
set -euo pipefail
cd "$(dirname "$0")"

IMAGE="${MISTER_ARM64_CROSS_IMAGE:-mister-magik-cross-armv7:ubuntu20-arm64}"
TARGET_DIR="${MISTER_ARM64_TARGET_DIR:-/private/tmp/mister-magik-arm64-target}"
RUST_TOOLCHAIN="${MISTER_ARM64_RUST_TOOLCHAIN:-$HOME/.rustup/toolchains/stable-aarch64-unknown-linux-gnu}"
TARGET=armv7-unknown-linux-gnueabihf

PROFILE=release-device
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
Native ARM64 Docker ARMv7 build:
  ./build-arm64-docker.sh              → release-device
  ./build-arm64-docker.sh --opt2       → release-opt2
  ./build-arm64-docker.sh --opts       → release-opts
  ./build-arm64-docker.sh --incr       → release-incr
  ./build-arm64-docker.sh --device     → release-device
  ./build-arm64-docker.sh --all-scenes → compile every Slint bench scene
  ./build-arm64-docker.sh --ui-scope S → launcher | arcade | all

One-time host setup:
  rustup toolchain add stable-aarch64-unknown-linux-gnu --profile minimal --force-non-host
  rustup target add armv7-unknown-linux-gnueabihf --toolchain stable-aarch64-unknown-linux-gnu
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
      echo "ERROR: --video is not supported by build-arm64-docker.sh yet; use build-arm.sh --video." >&2
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
if ! command -v docker >/dev/null 2>&1; then
  echo "ERROR: Docker is not installed or not on PATH." >&2
  exit 1
fi

mkdir -p "$TARGET_DIR"

echo "==> Building linux/arm64 cross image: $IMAGE"
docker build --platform linux/arm64 -f Dockerfile.cross-armv7 -t "$IMAGE" .

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

echo "==> docker build profile=$PROFILE ui_scope=$UI_SCOPE features=$FEATURE_LIST"
echo "==> target dir: $TARGET_DIR"
docker run --rm --platform linux/arm64 \
  --user "$(id -u):$(id -g)" \
  -e CARGO_HOME=/cargo \
  -e CARGO_TARGET_DIR=/target \
  -e MISTER_UI_BUILD_SCOPE="$UI_SCOPE" \
  -e RUSTC_WRAPPER= \
  -e RUSTFLAGS="$CONTAINER_RUSTFLAGS" \
  -e SLINT_FONT_SIZES="${SLINT_FONT_SIZES:-8,16,24,32}" \
  -v "$HOME/.cargo:/cargo" \
  -v "$RUST_TOOLCHAIN:/rust:ro" \
  -v "$PWD:/project" \
  -v "$TARGET_DIR:/target" \
  -w /project \
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
