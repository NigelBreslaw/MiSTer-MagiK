#!/usr/bin/env bash
# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

# Compatibility UI selector. MagiK modes use the canonical mode switcher.
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
MISTER="${MISTER:-$ROOT/scripts/mister}"

case "${1:-}" in
  -magik) exec "$ROOT/scripts/magik-mode.sh" public ;;
  -magik-dev) exec "$ROOT/scripts/magik-mode.sh" dev ;;
  -stock) exec "$ROOT/scripts/magik-mode.sh" stock ;;
  -zaparoo)
    "$MISTER" run '
set -e
source=/media/fat/MiSTer.ini.zaparoo
target=/media/fat/MiSTer.ini
tmp=/media/fat/.MiSTer.ini.frontend-switch
trap '\''rm -f "$tmp"'\'' EXIT
test -s "$source"
cp "$source" "$tmp"
sync
mv "$tmp" "$target"
sync
trap - EXIT
'
    "$MISTER" reboot-wait --raw
    ;;
  -h|--help|"")
    echo "usage: scripts/switch-ui.sh <-zaparoo|-magik|-magik-dev|-stock>"
    ;;
  *)
    echo "ERROR: expected -zaparoo, -magik, -magik-dev, or -stock" >&2
    exit 2
    ;;
esac
