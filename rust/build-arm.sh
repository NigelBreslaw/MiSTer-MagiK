#!/bin/bash
# Cross-compile the native MiSTer frontend for armv7 (the DE10-Nano's ARM core).
#
# Profiles (see rust/BUILD.md):
#   ./build-arm.sh              → release (fast daily)
#   ./build-arm.sh --device     → release-device (fat LTO + NEON, ship to MiSTer)
#   ./build-arm.sh --fast       → alias for release
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
for arg in "$@"; do
  case "$arg" in
    --device|--release-device) PROFILE=release-device ;;
    --fast|--release) PROFILE=release ;;
    -h|--help)
      sed -n '4,7p' "$0" | sed 's/^# \{0,1\}//'
      exit 0
      ;;
  esac
done

export DOCKER_DEFAULT_PLATFORM=linux/amd64
export SLINT_FONT_SIZES="${SLINT_FONT_SIZES:-16,32,48}"

if ! command -v docker >/dev/null 2>&1 || ! docker info >/dev/null 2>&1; then
  echo "ERROR: Docker is not running — cross needs it for armv7 builds." >&2
  exit 1
fi

if [ "$PROFILE" = release-device ]; then
  export RUSTFLAGS="${RUSTFLAGS:+$RUSTFLAGS }-C target-cpu=cortex-a9 -C target-feature=+neon,+vfp3"
  echo "==> cross build profile=release-device (fat LTO + NEON)"
else
  unset RUSTFLAGS
  echo "==> cross build profile=release (thin LTO, fast)"
fi

BUILD_LOG="$(mktemp)"
trap 'rm -f "$BUILD_LOG"' EXIT
if ! cross build --target armv7-unknown-linux-gnueabihf --profile "$PROFILE" 2>&1 | tee "$BUILD_LOG"; then
  exit 1
fi
if grep -q 'Falling back to `cargo` on the host' "$BUILD_LOG"; then
  echo "ERROR: cross fell back to host cargo (Docker not used). Check Docker and run from rust/." >&2
  exit 1
fi
echo "==> cross build OK"
