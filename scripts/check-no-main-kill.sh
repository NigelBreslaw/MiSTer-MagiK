#!/usr/bin/env bash
# Guard against scripts that kill Main_MiSTer and bypass MagiK launcher ownership.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

if rg -n 'kill -9 .*pidof (MiSTer|MiSTer_MagiK)|kill -9 .*\$\(pidof (MiSTer|MiSTer_MagiK)\)' \
  "$ROOT/scripts" \
  "$ROOT/magik-gui/src" \
  "$ROOT/magik-gui/ui/bench/README.md" \
  "$ROOT/tools/mister/src/main.rs"; then
  cat >&2 <<'EOF'
ERROR: scripts/docs must not kill MiSTer or MiSTer_MagiK.

Killing Main bypasses the MagiK fork's launcher ownership state and can leave
the stock OSD/input path over the Slint UI. Stop only mister-magik-fb, or reboot
through scripts/mister reboot-wait when display ownership is confused.
EOF
  exit 1
fi
