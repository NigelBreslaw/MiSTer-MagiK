#!/usr/bin/env bash
# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

# Build only the Slint-free ARM catalog builder.
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
export CROSS_CONFIG="$ROOT/apps/mister/Cross.toml"
export CARGO_TARGET_DIR=target
if [[ "$#" -ne 0 ]]; then
  echo "catalog builder build is flag-free" >&2
  exit 2
fi
exec "$ROOT/scripts/agent" build catalog-builder
