#!/usr/bin/env bash
# Cross-build the standalone MiSTer MagiK boot/network agent.
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
MANIFEST="$HERE/tools/magik-agent/Cargo.toml"
TARGET=armv7-unknown-linux-gnueabihf
BIN="$HERE/tools/magik-agent/target/$TARGET/release/mister-magik-agent"

BACKEND="${MISTER_ARM_BUILD_BACKEND:-auto}"
case "$BACKEND" in
  auto|apple-container|cross) ;;
  *)
    echo "ERROR: invalid MISTER_ARM_BUILD_BACKEND=$BACKEND (expected auto, apple-container, or cross)" >&2
    exit 2
    ;;
esac

build_with_apple_container() {
  local image target_dir mirror_target_dir cargo_cache rust_toolchain dockerfile image_stamp
  image="${MISTER_APPLE_CONTAINER_IMAGE:-mister-magik-cross-armv7:ubuntu20-arm64}"
  target_dir="${MISTER_APPLE_CONTAINER_AGENT_TARGET_DIR:-/private/tmp/mister-magik-agent-apple-container-target}"
  mirror_target_dir="$HERE/tools/magik-agent/target"
  cargo_cache="${MISTER_APPLE_CONTAINER_CARGO_HOME:-$HOME/.cargo}"
  rust_toolchain="${MISTER_ARM64_RUST_TOOLCHAIN:-$HOME/.rustup/toolchains/stable-aarch64-unknown-linux-gnu}"
  dockerfile="$HERE/magik-gui/Dockerfile.cross-armv7"
  image_stamp="${MISTER_APPLE_CONTAINER_IMAGE_STAMP:-/private/tmp/mister-magik-apple-container-target.image.sha256}"

  if [ "$(uname -s)" != Darwin ] || [ "$(uname -m)" != arm64 ]; then
    echo "ERROR: Apple-container backend requires arm64 macOS; got $(uname -s)/$(uname -m)." >&2
    exit 1
  fi
  if ! command -v container >/dev/null 2>&1; then
    echo "ERROR: Apple container is not installed or not on PATH." >&2
    exit 1
  fi
  if [ ! -x "$rust_toolchain/bin/cargo" ]; then
    echo "ERROR: missing linux/aarch64 Rust toolchain at $rust_toolchain" >&2
    echo "Install it with:" >&2
    echo "  rustup toolchain add stable-aarch64-unknown-linux-gnu --profile minimal --force-non-host" >&2
    echo "  rustup target add $TARGET --toolchain stable-aarch64-unknown-linux-gnu" >&2
    exit 1
  fi
  if [ ! -d "$rust_toolchain/lib/rustlib/$TARGET" ]; then
    echo "ERROR: missing $TARGET std for stable-aarch64-unknown-linux-gnu" >&2
    echo "Install it with:" >&2
    echo "  rustup target add $TARGET --toolchain stable-aarch64-unknown-linux-gnu" >&2
    exit 1
  fi

  mkdir -p "$target_dir" "$cargo_cache"

  local hash expected_stamp existing_stamp
  hash="$(shasum -a 256 "$dockerfile" | awk '{print $1}')"
  expected_stamp="$image  $hash"
  existing_stamp="$(cat "$image_stamp" 2>/dev/null || true)"
  if [ "$existing_stamp" != "$expected_stamp" ] ||
    ! container run --arch arm64 --rm "$image" true >/dev/null 2>&1; then
    echo "==> building linux/arm64 cross image with Apple container: $image" >&2
    container build --arch arm64 --file "$dockerfile" --tag "$image" "$HERE/magik-gui" >&2
    printf '%s\n' "$expected_stamp" >"$image_stamp"
  else
    echo "==> linux/arm64 cross image is current: $image" >&2
  fi

  echo "==> build backend: apple-container" >&2
  echo "==> target dir: $target_dir" >&2
  container run --arch arm64 --rm \
    --env CARGO_HOME=/cargo \
    --env CARGO_TARGET_DIR=/target \
    --env RUSTC_WRAPPER= \
    --env RUSTFLAGS="${RUSTFLAGS:+$RUSTFLAGS }-D warnings -C target-cpu=cortex-a9" \
    --volume "$cargo_cache:/cargo" \
    --volume "$rust_toolchain:/rust:ro" \
    --volume "$HERE:/project" \
    --volume "$target_dir:/target" \
    --workdir /project/tools/magik-agent \
    "$image" \
    sh -lc 'PATH=/rust/bin:$PATH cargo build --target armv7-unknown-linux-gnueabihf --release --locked' >&2

  local built mirror
  built="$target_dir/$TARGET/release/mister-magik-agent"
  if [ ! -f "$built" ]; then
    echo "ERROR: expected binary not found: $built" >&2
    exit 1
  fi
  mirror="$mirror_target_dir/$TARGET/release/mister-magik-agent"
  mkdir -p "$(dirname "$mirror")"
  cp "$built" "$mirror"
}

build_with_cross() {
  export DOCKER_DEFAULT_PLATFORM=linux/amd64
  export RUSTC_WRAPPER=""
  export RUSTFLAGS="${RUSTFLAGS:+$RUSTFLAGS }-D warnings -C target-cpu=cortex-a9"

  cross build \
    --manifest-path "$MANIFEST" \
    --target "$TARGET" \
    --release
}

if [ "$BACKEND" = auto ] || [ "$BACKEND" = apple-container ]; then
  if [ "$(uname -s)" = Darwin ] && [ "$(uname -m)" = arm64 ]; then
    build_with_apple_container
  elif [ "$BACKEND" = apple-container ]; then
    echo "ERROR: Apple-container backend requires arm64 macOS; got $(uname -s)/$(uname -m)." >&2
    exit 1
  else
    build_with_cross
  fi
else
  build_with_cross
fi

if [ ! -x "$BIN" ]; then
  echo "ERROR: expected binary not found: $BIN" >&2
  exit 1
fi
echo "$BIN"
