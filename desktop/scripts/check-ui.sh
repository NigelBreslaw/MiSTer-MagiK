#!/usr/bin/env bash
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
DESKTOP_DIR="$(cd "$HERE/.." && pwd)"

slint-viewer --check "$DESKTOP_DIR/ui/main.slint"
