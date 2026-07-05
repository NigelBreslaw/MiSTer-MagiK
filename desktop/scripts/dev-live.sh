#!/usr/bin/env bash
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
DESKTOP_DIR="$(cd "$HERE/.." && pwd)"
export RUSTC_WRAPPER=""
export SLINT_BACKEND="${SLINT_BACKEND:-winit-skia}"
export SLINT_EMIT_DEBUG_INFO="${SLINT_EMIT_DEBUG_INFO:-1}"

features="live-ui"
if [ "${MISTER_DESKTOP_MCP:-0}" = 1 ]; then
  export SLINT_MCP_PORT="${SLINT_MCP_PORT:-9315}"
  features="slint/mcp,live-ui"
fi

exec cargo run --release --manifest-path "$DESKTOP_DIR/Cargo.toml" --features "$features" "$@"
