#!/usr/bin/env bash
# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

export MISTER_DESKTOP_MCP=1
export SLINT_MCP_PORT="${SLINT_MCP_PORT:-9315}"

exec "$HERE/dev-live.sh" "$@"
