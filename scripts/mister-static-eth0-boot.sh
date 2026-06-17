#!/usr/bin/env bash
# Install/remove a reversible early static eth0 boot experiment on MiSTer.
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
MISTER="$HERE/scripts/mister"
REMOTE_SCRIPT="/etc/init.d/S11staticeth0"
MARKER="/media/fat/mister-magik/S11staticeth0.experiment"

usage() {
  cat <<EOF
Usage: scripts/mister-static-eth0-boot.sh <install|remove|status>

Adds /etc/init.d/S11staticeth0 so rcS tries direct static eth0 setup immediately
after S10udev, bypassing dhcpcd's discovery path for this experiment.
EOF
}

make_payload() {
  local out="$1"
  cat >"$out" <<'EOF'
#!/bin/sh

case "$1" in
  start)
    echo "Starting static eth0 experiment..."
    /sbin/ifconfig eth0 192.168.1.117 netmask 255.255.255.0 up || exit 0
    /sbin/route add default gw 192.168.1.1 eth0 2>/dev/null || true
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
  "$MISTER" put "$tmp" /tmp/S11staticeth0 >/dev/null
  rm -f "$tmp"
  "$MISTER" run "
set -eu
mkdir -p /media/fat/mister-magik
mount -o remount,rw /
cp /tmp/S11staticeth0 '$REMOTE_SCRIPT'
chmod 755 '$REMOTE_SCRIPT'
echo installed >'$MARKER'
sync
mount -o remount,ro / || true
echo static_eth0=installed
ls -l '$REMOTE_SCRIPT'
"
}

remote_remove() {
  "$MISTER" run "
set -eu
mount -o remount,rw /
if [ -f '$REMOTE_SCRIPT' ]; then
  rm -f '$REMOTE_SCRIPT'
  echo static_eth0=removed
else
  echo static_eth0=not-installed
fi
rm -f '$MARKER'
sync
mount -o remount,ro / || true
"
}

remote_status() {
  "$MISTER" run "
if [ -f '$REMOTE_SCRIPT' ]; then
  echo static_eth0=installed
  ls -l '$REMOTE_SCRIPT'
else
  echo static_eth0=not-installed
fi
if [ -f '$MARKER' ]; then
  cat '$MARKER'
fi
"
}

case "${1:-}" in
  install) remote_install ;;
  remove) remote_remove ;;
  status) remote_status ;;
  -h|--help|"") usage ;;
  *) echo "unknown action: $1" >&2; usage >&2; exit 2 ;;
esac
