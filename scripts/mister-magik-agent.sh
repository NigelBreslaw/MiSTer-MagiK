#!/usr/bin/env bash
# Install/remove the standalone MiSTer MagiK boot/network agent.
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
MISTER="$HERE/scripts/mister"
REMOTE_BIN="/media/fat/mister-magik/mister-magik-agent"
REMOTE_SCRIPT="/etc/init.d/S00magik-agent"
FASTNET_SCRIPT="/etc/init.d/S00fastnet"
FASTNET_DISABLED="/etc/init.d/disabled-S00fastnet.magik-agent"
MARKER="/media/fat/mister-magik/S00magik-agent.experiment"

usage() {
  cat <<EOF
Usage: scripts/mister-magik-agent.sh <build|install|remove|status|log>

Installs a compiled early network agent as /etc/init.d/S00magik-agent and
temporarily disables the shell FastNet service so timings are isolated.

SD-card recovery if SSH is lost: delete /etc/init.d/S00magik-agent. If needed,
rename /etc/init.d/disabled-S00fastnet.magik-agent back to /etc/init.d/S00fastnet.
EOF
}

build_agent() {
  "$HERE/scripts/build-mister-agent.sh"
}

install_agent() {
  local bin
  bin="$(build_agent)"
  "$MISTER" run "mkdir -p /media/fat/mister-magik" >/dev/null
  "$MISTER" put "$bin" "$REMOTE_BIN.upload" >/dev/null
  "$MISTER" run "
set -eu
mv '$REMOTE_BIN.upload' '$REMOTE_BIN'
chmod 755 '$REMOTE_BIN'
mount -o remount,rw /
if [ -f '$FASTNET_SCRIPT' ] && [ ! -f '$FASTNET_DISABLED' ]; then
  mv '$FASTNET_SCRIPT' '$FASTNET_DISABLED'
fi
cat >'$REMOTE_SCRIPT' <<'EOS'
#!/bin/sh
case \"\$1\" in
  start)
    /media/fat/mister-magik/mister-magik-agent net-boot >/tmp/mister-magik-agent.boot.out 2>&1 &
    ;;
  stop)
    ;;
  *)
    echo \"Usage: \$0 {start|stop}\"
    exit 1
    ;;
esac
exit 0
EOS
chmod 755 '$REMOTE_SCRIPT'
echo installed >'$MARKER'
sync
mount -o remount,ro / || true
echo magik_agent=installed
ls -l '$REMOTE_SCRIPT' '$REMOTE_BIN'
"
}

remove_agent() {
  "$MISTER" run "
set -eu
mount -o remount,rw /
rm -f '$REMOTE_SCRIPT'
if [ -f '$FASTNET_DISABLED' ]; then
  mv '$FASTNET_DISABLED' '$FASTNET_SCRIPT'
fi
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
if [ -f '$FASTNET_SCRIPT' ]; then
  echo fastnet=enabled
fi
if [ -f '$FASTNET_DISABLED' ]; then
  echo fastnet=disabled-for-agent
fi
if [ -x '$REMOTE_BIN' ]; then
  ls -l '$REMOTE_BIN'
fi
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
