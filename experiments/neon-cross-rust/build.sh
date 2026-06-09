#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "$0")"

export DOCKER_DEFAULT_PLATFORM=linux/amd64
export RUSTC_WRAPPER=""
export RUSTFLAGS="${RUSTFLAGS:+$RUSTFLAGS }-C target-cpu=cortex-a9 -C target-feature=+neon"

cross build --target armv7-unknown-linux-gnueabihf "$@"
