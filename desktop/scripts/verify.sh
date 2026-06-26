#!/usr/bin/env bash
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
DESKTOP_DIR="$(cd "$HERE/.." && pwd)"
export RUSTC_WRAPPER=""

cargo test --manifest-path "$DESKTOP_DIR/Cargo.toml"
cargo check --manifest-path "$DESKTOP_DIR/Cargo.toml" --no-default-features --features compiled-ui
"$HERE/check-ui.sh"
slint-viewer --screenshot /private/tmp/mister-magik-desktop.png "$DESKTOP_DIR/ui/main.slint"

if [ "${MISTER_DESKTOP_MCP_SMOKE:-0}" = 1 ]; then
  "$HERE/mcp-smoke.sh"
fi
