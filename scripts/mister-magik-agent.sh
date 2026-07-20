#!/usr/bin/env bash
# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

# Install/remove the standalone MiSTer MagiK boot/network agent.
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
MISTER="$HERE/scripts/mister"
REMOTE_BIN="/media/fat/mister-magik-dev/mister-magik-agent"
REMOTE_SCRIPT="/etc/init.d/S00magik-agent"
REMOTE_TOKEN="/media/fat/mister-magik-dev/agent.token"
MARKER="/media/fat/mister-magik-dev/S00magik-agent.experiment"

usage() {
  cat <<EOF
Usage: scripts/mister-magik-agent.sh <build|install|remove|status|log>

The normal CLI installs or upgrades the control agent automatically without
changing the MiSTer's network configuration.

The device token lives at /media/fat/mister-magik-dev/agent.token. The shared
host copy is managed outside the worktree under ~/.config/mister-magik/.

SD-card recovery if SSH is lost: delete /etc/init.d/S00magik-agent.
EOF
}

build_agent() {
  "$HERE/scripts/build-mister-agent.sh"
}

install_agent() {
  "$MISTER" connected
}

remove_agent() {
  MISTER_SKIP_AGENT_BOOTSTRAP=1 "$MISTER" run "
set -eu
mount -o remount,rw /
if [ -x '$REMOTE_SCRIPT' ]; then '$REMOTE_SCRIPT' stop || true; else kill \$(pidof mister-magik-agent 2>/dev/null || true) 2>/dev/null || true; fi
rm -f '$REMOTE_SCRIPT'
rm -f '$MARKER'
sync
mount -o remount,ro / || true
echo magik_agent=removed
"
}

status_agent() {
  "$MISTER" run "
if [ -f '$REMOTE_SCRIPT' ]; then
  echo magik_agent=installed
  ls -l '$REMOTE_SCRIPT'
else
  echo magik_agent=not-installed
fi
if [ -x '$REMOTE_BIN' ]; then
  ls -l '$REMOTE_BIN'
fi
if [ -s '$REMOTE_TOKEN' ]; then
  echo agent_token=installed
else
  echo agent_token=missing
fi
echo magik_agent_pid=\$(pidof mister-magik-agent 2>/dev/null || true)
"
}

log_agent() {
  "$MISTER" run "cat /tmp/mister-magik-agent.log 2>/dev/null || true"
}

case "${1:-}" in
  build) build_agent ;;
  install) install_agent ;;
  remove) remove_agent ;;
  status) status_agent ;;
  log) log_agent ;;
  -h|--help|"") usage ;;
  *) echo "unknown action: $1" >&2; usage >&2; exit 2 ;;
esac
