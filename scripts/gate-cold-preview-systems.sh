#!/usr/bin/env bash
# Gate first-preview readiness for installed screenshot-pack systems.
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

exec "$HERE/scripts/profile-cold-preview-systems.sh" "$@" --require-pass
