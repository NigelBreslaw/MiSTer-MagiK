#!/usr/bin/env bash
# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
cargo run \
  --quiet \
  --manifest-path "$ROOT/apps/mister/Cargo.toml" \
  --features asset-tools \
  --example generate-bitmap-fonts \
  -- \
  "$ROOT/apps/mister/ui/fonts" \
  "$ROOT/private/magik-assets" \
  "$ROOT/apps/mister/assets/fonts"
