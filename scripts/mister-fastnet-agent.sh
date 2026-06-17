#!/usr/bin/env bash
# Install/remove a reversible early FastNet agent on MiSTer.
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
MISTER="$HERE/scripts/mister"
REMOTE_SCRIPT="/etc/init.d/S03fastnet"
MARKER="/media/fat/mister-magik/S03fastnet.experiment"

usage() {
  cat <<EOF
Usage: scripts/mister-fastnet-agent.sh <install|remove|status|log>

Installs an early background FastNet agent. It starts before normal rcS network
services, waits for eth0 to exist, repeatedly applies the known static IPv4
configuration, emits timestamped logs, and leaves normal dhcpcd/sshd in place.

SD-card recovery if SSH is lost: delete /etc/init.d/S03fastnet from the MiSTer
Linux root image.
EOF
}

make_payload() {
  local out="$1"
  cat >"$out" <<'EOF'
#!/bin/sh

LOG=/tmp/mister-magik-fastnet.log
IP=192.168.1.117
NETMASK=255.255.255.0
GW=192.168.1.1

stamp() {
  awk '{print $1}' /proc/uptime 2>/dev/null || echo "?"
}

log() {
  echo "$(stamp) fastnet $*" >>"$LOG"
}

configure_once() {
  if [ ! -d /sys/class/net/eth0 ]; then
    log "eth0_missing"
    return 1
  fi
  /sbin/ifconfig eth0 "$IP" netmask "$NETMASK" up >>"$LOG" 2>&1 || {
    log "ifconfig_failed"
    return 1
  }
  /sbin/route add default gw "$GW" eth0 >>"$LOG" 2>&1 || true
  if command -v arping >/dev/null 2>&1; then
    arping -A -c 1 -I eth0 "$IP" >>"$LOG" 2>&1 || true
  fi
  carrier="$(cat /sys/class/net/eth0/carrier 2>/dev/null || echo "?")"
  operstate="$(cat /sys/class/net/eth0/operstate 2>/dev/null || echo "?")"
  log "configured carrier=$carrier operstate=$operstate"
  return 0
}

worker() {
  : >"$LOG"
  log "worker_start pid=$$"
  i=0
  while [ "$i" -lt 80 ]; do
    configure_once || true
    carrier="$(cat /sys/class/net/eth0/carrier 2>/dev/null || echo 0)"
    if [ "$carrier" = "1" ]; then
      log "carrier_ready"
      exit 0
    fi
    i=$((i + 1))
    sleep 0.25
  done
  log "gave_up"
}

case "$1" in
  start)
    worker &
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
  "$MISTER" put "$tmp" /tmp/S03fastnet >/dev/null
  rm -f "$tmp"
  "$MISTER" run "
set -eu
mkdir -p /media/fat/mister-magik
mount -o remount,rw /
cp /tmp/S03fastnet '$REMOTE_SCRIPT'
chmod 755 '$REMOTE_SCRIPT'
echo installed >'$MARKER'
sync
mount -o remount,ro / || true
echo fastnet_agent=installed
ls -l '$REMOTE_SCRIPT'
"
}

remote_remove() {
  "$MISTER" run "
set -eu
mount -o remount,rw /
if [ -f '$REMOTE_SCRIPT' ]; then
  rm -f '$REMOTE_SCRIPT'
  echo fastnet_agent=removed
else
  echo fastnet_agent=not-installed
fi
rm -f '$MARKER'
sync
mount -o remount,ro / || true
"
}

remote_status() {
  "$MISTER" run "
if [ -f '$REMOTE_SCRIPT' ]; then
  echo fastnet_agent=installed
  ls -l '$REMOTE_SCRIPT'
else
  echo fastnet_agent=not-installed
fi
if [ -f '$MARKER' ]; then
  cat '$MARKER'
fi
"
}

remote_log() {
  "$MISTER" run "cat /tmp/mister-magik-fastnet.log 2>/dev/null || true"
}

case "${1:-}" in
  install) remote_install ;;
  remove) remote_remove ;;
  status) remote_status ;;
  log) remote_log ;;
  -h|--help|"") usage ;;
  *) echo "unknown action: $1" >&2; usage >&2; exit 2 ;;
esac
