#!/usr/bin/env bash
# Cross-build the standalone MiSTer MagiK boot/network agent.
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
MANIFEST="$HERE/tools/magik-agent/Cargo.toml"

export DOCKER_DEFAULT_PLATFORM=linux/amd64
export RUSTC_WRAPPER=""
export RUSTFLAGS="${RUSTFLAGS:+$RUSTFLAGS }-C target-cpu=cortex-a9"

cross build \
  --manifest-path "$MANIFEST" \
  --target armv7-unknown-linux-gnueabihf \
  --release

BIN="$HERE/tools/magik-agent/target/armv7-unknown-linux-gnueabihf/release/mister-magik-agent"
if [ ! -x "$BIN" ]; then
  echo "ERROR: expected binary not found: $BIN" >&2
  exit 1
fi
echo "$BIN"
