#!/usr/bin/env bash
# Sweep dirty_band band-pct on MiSTer — wrapper for uv + paramiko.
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
exec uv run python "$HERE/scripts/bench_dirty_band.py" "$@"
