#!/usr/bin/env bash
# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"

forbidden='latch_readiness_json\\t|agent magik \{action\} ok after'
if rg -n "$forbidden" \
  "$ROOT/apps/mister/src/main.rs" \
  "$ROOT/agent-cli/src/host/mod.rs"; then
  echo "default device workflows must not print duplicate or full-result success JSON" >&2
  exit 1
fi

echo "compact diagnostic output check ok"
