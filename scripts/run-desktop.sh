#!/usr/bin/env bash
# Run the Slint UI on this desktop machine (macOS/Linux) via uv.
#   scripts/run-desktop.sh              # open the window normally
#   MISTER_MAGIC_CHECK=1 scripts/...    # headless self-test, no display needed
#   MISTER_MAGIC_SMOKE=1 scripts/...    # open, render, then auto-quit
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$HERE"
export PATH="$HOME/.local/bin:$PATH"
exec uv run python src/main.py "$@"
