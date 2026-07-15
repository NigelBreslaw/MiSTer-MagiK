#!/usr/bin/env bash
# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

# Build only the Slint-free ARM catalog builder.
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
export CROSS_CONFIG="$ROOT/magik-gui/Cross.toml"
export CARGO_TARGET_DIR=target
exec "$ROOT/magik-gui/build-arm.sh" --catalog-builder "$@"
