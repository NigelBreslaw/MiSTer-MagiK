#!/usr/bin/env bash
# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
matches="$(rg -n 'printf .*MiSTer_cmd|> /dev/MiSTer_cmd' "$ROOT/scripts" \
  --glob '!check-no-direct-main-fifo.sh' \
  --glob '!fs-fault-reset-testing.sh' || true)"
if [ -n "$matches" ]; then
  echo "maintained scripts must use typed Rust host operations, not /dev/MiSTer_cmd" >&2
  printf '%s\n' "$matches" >&2
  exit 1
fi
echo "direct Main FIFO writer check ok"
