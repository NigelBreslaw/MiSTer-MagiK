#!/usr/bin/env bash
# Install/remove a reversible early sshd boot experiment on MiSTer.
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
MISTER="$HERE/scripts/mister"
REMOTE_SCRIPT="/etc/init.d/S04fastsshd"
MARKER="/media/fat/mister-magik/S04fastsshd.experiment"

usage() {
  cat <<EOF
Usage: scripts/mister-fastsshd-boot.sh <install|remove|status|log>

Installs an early sshd starter. It runs before normal rcS network services,
ensures host keys exist, starts sshd if it is not already running, and logs
timestamps to /tmp/mister-magik-fastsshd.log.

SD-card recovery if SSH is lost: delete /etc/init.d/S04fastsshd from the MiSTer
Linux root image.
EOF
}

make_payload() {
  local out="$1"
  cat >"$out" <<'EOF'
#!/bin/sh

LOG=/tmp/mister-magik-fastsshd.log

stamp() {
  awk '{print $1}' /proc/uptime 2>/dev/null || echo "?"
}

log() {
  echo "$(stamp) fastsshd $*" >>"$LOG"
}

case "$1" in
  start)
    : >"$LOG"
    log "start"
    mkdir -p /var/run /run /var/empty /var/lock
    if ! pidof sshd >/dev/null 2>&1; then
      /usr/bin/ssh-keygen -A >>"$LOG" 2>&1 || log "ssh_keygen_failed"
      /usr/sbin/sshd >>"$LOG" 2>&1 && log "sshd_started" || log "sshd_failed"
    else
      log "sshd_already_running"
    fi
    ;;
  stop)
    ;;
  *)
    echo "Usage: $0 {start|stop}"
    exit 1
    ;;
esac

exit 0
EOF
}

remote_install() {
  local tmp
  tmp="$(mktemp)"
  make_payload "$tmp"
  "$MISTER" put "$tmp" /tmp/S04fastsshd >/dev/null
  rm -f "$tmp"
  "$MISTER" run "
set -eu
mkdir -p /media/fat/mister-magik
mount -o remount,rw /
cp /tmp/S04fastsshd '$REMOTE_SCRIPT'
chmod 755 '$REMOTE_SCRIPT'
echo installed >'$MARKER'
sync
mount -o remount,ro / || true
echo fastsshd=installed
ls -l '$REMOTE_SCRIPT'
"
}

remote_remove() {
  "$MISTER" run "
set -eu
mount -o remount,rw /
if [ -f '$REMOTE_SCRIPT' ]; then
  rm -f '$REMOTE_SCRIPT'
  echo fastsshd=removed
else
  echo fastsshd=not-installed
fi
rm -f '$MARKER'
sync
mount -o remount,ro / || true
"
}

remote_status() {
  "$MISTER" run "
if [ -f '$REMOTE_SCRIPT' ]; then
  echo fastsshd=installed
  ls -l '$REMOTE_SCRIPT'
else
  echo fastsshd=not-installed
fi
if [ -f '$MARKER' ]; then
  cat '$MARKER'
fi
"
}

remote_log() {
  "$MISTER" run "cat /tmp/mister-magik-fastsshd.log 2>/dev/null || true"
}

case "${1:-}" in
  install) remote_install ;;
  remove) remote_remove ;;
  status) remote_status ;;
  log) remote_log ;;
  -h|--help|"") usage ;;
  *) echo "unknown action: $1" >&2; usage >&2; exit 2 ;;
esac
