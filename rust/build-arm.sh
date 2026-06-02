#!/bin/bash
# Cross-compile the native MiSTer frontend for armv7 (the DE10-Nano's ARM core).
#
# Wraps `cross` with the settings the toolchain needs on an Apple-Silicon host
# (see AGENTS.md §12 for the full why):
#   - DOCKER_DEFAULT_PLATFORM=linux/amd64: the cross 0.2.5 image is x86_64-only,
#     so on arm64 Docker we must request the amd64 manifest (qemu-emulated).
#   - The sccache rustc-wrapper from ~/.cargo/config.toml is overridden to empty
#     in rust/.cargo/config.toml (its macOS path is invalid in the container).
#   - rust-toolchain.toml pins stable 1.88 + the armv7 target.
#
# One-time host setup (already done, documented here for reproducibility):
#   cargo install cross --locked
#   rustup toolchain add 1.88-x86_64-unknown-linux-gnu --profile minimal --force-non-host
#
# Usage: ./build-arm.sh [--release]   (defaults to --release)
set -euo pipefail
cd "$(dirname "$0")"

ARGS=("$@")
if [ ${#ARGS[@]} -eq 0 ]; then
    ARGS=(--release)
fi

export DOCKER_DEFAULT_PLATFORM=linux/amd64
exec cross build --target armv7-unknown-linux-gnueabihf "${ARGS[@]}"
