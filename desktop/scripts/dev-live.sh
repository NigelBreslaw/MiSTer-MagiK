#!/usr/bin/env bash
# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
DESKTOP_DIR="$(cd "$HERE/.." && pwd)"
export RUSTC_WRAPPER=""
export SLINT_BACKEND="${SLINT_BACKEND:-winit-skia}"
export SLINT_EMIT_DEBUG_INFO="${SLINT_EMIT_DEBUG_INFO:-1}"

features="live-ui,skia-renderer"
if [ "${MISTER_DESKTOP_MCP:-0}" = 1 ]; then
  export SLINT_MCP_PORT="${SLINT_MCP_PORT:-9315}"
  features="slint/mcp,live-ui,skia-renderer"
fi

exec cargo run --release --manifest-path "$DESKTOP_DIR/Cargo.toml" --features "$features" "$@"
