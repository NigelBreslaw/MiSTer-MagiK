#!/usr/bin/env bash
# Run the Arcade scroll profiler through the experimental Main-owned present route.
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

export MISTER_PRESENT_BACKEND="${MISTER_PRESENT_BACKEND:-main-flip-v1}"
export MISTER_PRESENT_FLIP_BUFFER_INDEX="${MISTER_PRESENT_FLIP_BUFFER_INDEX:-1}"

exec "$HERE/scripts/profile-arcade-scroll.sh" "$@"
