#!/usr/bin/env bash
# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

# Standalone Catalog V3 full-versus-delta rebuild benchmark and 10x gate.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SYSTEMS="${MISTER_CATALOG_BENCH_SYSTEMS:-30}"
GAMES="${MISTER_CATALOG_BENCH_GAMES_PER_SYSTEM:-200}"
LABEL="${1:-catalog-rebuild}"
STORAGE="$(mktemp -d "${TMPDIR:-/tmp}/mister-magik-rebuild-bench.XXXXXX")"
REPORT="$(mktemp "${TMPDIR:-/tmp}/mister-magik-rebuild-report.XXXXXX")"
trap 'rm -rf "$STORAGE"; rm -f "$REPORT"' EXIT

cargo run --release \
  --manifest-path "$ROOT/magik-gui/catalog/Cargo.toml" \
  --features builder \
  --bin catalog-lab -- \
  rebuild-bench "$STORAGE" "$SYSTEMS" "$GAMES" | tee "$REPORT"

python3 "$ROOT/scripts/checks/check-catalog-rebuild.py" "$LABEL" "$REPORT" 10.0
