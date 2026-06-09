#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "$0")"

export DOCKER_DEFAULT_PLATFORM=linux/amd64
export RUSTC_WRAPPER=""

cross build --target armv7-unknown-linux-gnueabihf "$@"
