#!/usr/bin/env bash
# Build only the Slint-free ARM catalog builder.
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
exec "$ROOT/magik-gui/build-arm.sh" --catalog-builder "$@"
