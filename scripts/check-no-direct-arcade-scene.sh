#!/usr/bin/env bash
# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

# Guard against resurrecting the removed direct `ui arcade` entrypoint.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

if rg -n \
  -e 'mister-magik-fb[[:space:]]+ui[[:space:]]+arcade' \
  -e 'mister-magic-fb[[:space:]]+ui[[:space:]]+arcade' \
  -e '\$REMOTE[[:space:]]+ui[[:space:]]+arcade' \
  -e "\$REMOTE[[:space:]]+ui[[:space:]]+'arcade'" \
  -e '\$REMOTE[[:space:]]+ui[[:space:]]+"arcade"' \
  -e "ui[[:space:]]+'arcade'" \
  -e 'ui[[:space:]]+"arcade"' \
  -e 'LauncherRunMode::Arcade' \
  -e 'REMOTE_SCENE="arcade"' \
  "$ROOT/scripts" \
  "$ROOT/docs" \
  "$ROOT/magik-gui/src" \
  "$ROOT/magik-gui/ui" \
  --glob '!check-no-direct-arcade-scene.sh' \
  --glob '!main.rs'; then
  cat >&2 <<'EOF'
ERROR: the direct `mister-magik-fb ui arcade` scene was removed.

Arcade benchmarks must run through MiSTer_MagiK supervising
`mister-magik-fb ui launcher 0`, with launcher.env selecting and locking the
real Arcade screen. Direct arcade launches can bypass Main's OSD/VT/input
suppression and contaminate the display path.
EOF
  exit 1
fi
