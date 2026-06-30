#!/usr/bin/env bash
# Gate wrapper for cold direct-to-system turbo preview coverage.
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

exec "$HERE/scripts/profile-cold-turbo-preview.sh" "$@" --require-pass
