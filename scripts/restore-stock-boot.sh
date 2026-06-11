#!/usr/bin/env bash
# Restore stock MiSTer boot.
#
# Keep stock /media/fat/MiSTer in inittab and disable MagiK's MiSTer.ini handoff.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
MISTER_IP="${MISTER_IP:?Set MISTER_IP}"
MISTER_PASS="${MISTER_PASS:-1}"

MISTER_IP="$MISTER_IP" MISTER_PASS="$MISTER_PASS" scripts/mister run '
set -e
mount -o remount,rw / 2>/dev/null || true
INI=/media/fat/MiSTer.ini
BACKUP=/media/fat/MiSTer.ini.bak
STAMP=$(date +%Y%m%d-%H%M%S 2>/dev/null || echo unknown)
SNAP="/media/fat/mister-magik/snapshots/$STAMP-restore-stock"
mkdir -p "$SNAP"
cp /etc/inittab "$SNAP/inittab" 2>/dev/null || true
cp "$INI" "$SNAP/MiSTer.ini" 2>/dev/null || true
cp "$BACKUP" "$SNAP/MiSTer.ini.bak" 2>/dev/null || true
ps > "$SNAP/ps.txt" 2>/dev/null || true
cat /sys/module/MiSTer_fb/parameters/mode > "$SNAP/fb-mode.txt" 2>/dev/null || true
cp /tmp/mister-magik-main.log "$SNAP/mister-magik-main.log" 2>/dev/null || true
echo "snapshot: $SNAP"

tmp=/tmp/inittab.stock
awk '"'"'
BEGIN { wrote = 0 }
/^::sysinit:\/media\/fat\/MiSTer[[:space:]]*&/ {
  if (!wrote) {
    print "::sysinit:/media/fat/MiSTer &"
    wrote = 1
  }
  next
}
/^::sysinit:\/media\/fat\/MiSTer_MagiK/ { next }
/^::sysinit:\/media\/fat\/mister-magik\/boot\.sh/ { next }
{ print }
END {
  if (!wrote) print "::sysinit:/media/fat/MiSTer &"
}
'"'"' /etc/inittab > "$tmp"
cp "$tmp" /etc/inittab

kill -9 $(pidof mister-magik-fb) 2>/dev/null || true
kill -9 $(pidof MiSTer_MagiK) 2>/dev/null || true
sync

echo "=== restored inittab ==="
grep -n "sysinit" /etc/inittab | grep -E "MiSTer|MagiK|boot.sh" || true
'

MISTER_IP="$MISTER_IP" MISTER_PASS="$MISTER_PASS" scripts/mister ini-restore-stock
MISTER_IP="$MISTER_IP" MISTER_PASS="$MISTER_PASS" scripts/mister reboot-wait
echo "Stock MiSTer boot restored. MiSTer.ini backup left untouched at /media/fat/MiSTer.ini.bak if present."
