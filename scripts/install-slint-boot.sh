#!/usr/bin/env bash
# Install update_all-compatible Magik boot through MiSTer's native main= hook.
#
# Stock /media/fat/MiSTer stays as the only inittab menu entry. It reads
# MiSTer.ini and re-execs /media/fat/MiSTer_MagiK.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

MISTER_IP="${MISTER_IP:?Set MISTER_IP}"
MISTER_PASS="${MISTER_PASS:-1}"

echo "==> Configure device (stock inittab + MiSTer.ini main=MiSTer_MagiK)"
MISTER_IP="$MISTER_IP" MISTER_PASS="$MISTER_PASS" scripts/mister run '
set -e
if [ ! -x /media/fat/MiSTer_MagiK ]; then
  echo "ERROR: /media/fat/MiSTer_MagiK is missing or not executable"
  echo "Run scripts/deploy-main-mister-experiment.sh first."
  exit 1
fi

INI=/media/fat/MiSTer.ini
STAMP=$(date +%Y%m%d-%H%M%S 2>/dev/null || echo unknown)
SNAP="/media/fat/mister-magik/snapshots/$STAMP-install"
mkdir -p "$SNAP"
cp /etc/inittab "$SNAP/inittab" 2>/dev/null || true
cp "$INI" "$SNAP/MiSTer.ini" 2>/dev/null || true
ps > "$SNAP/ps.txt" 2>/dev/null || true
cat /sys/module/MiSTer_fb/parameters/mode > "$SNAP/fb-mode.txt" 2>/dev/null || true
cp /tmp/mister-magik-main.log "$SNAP/mister-magik-main.log" 2>/dev/null || true
echo "snapshot: $SNAP"
if [ ! -f "$INI.bak" ]; then cp "$INI" "$INI.bak"; fi

# Stock MiSTer starts first, then MiSTer.ini main= hands off to MiSTer_MagiK.
mount -o remount,rw / 2>/dev/null || true
tmp=/tmp/inittab.magik
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
echo "inittab ensured -> stock MiSTer"
sync

echo "=== post-install inittab ==="
grep -n "sysinit" /etc/inittab | grep -E "MiSTer|MagiK|boot.sh" || true
echo "=== post-install processes ==="
ps | grep -E "[M]iSTer|[M]iSTer_MagiK|[m]ister-magik-fb" || true
echo "=== post-install fb mode ==="
cat /sys/module/MiSTer_fb/parameters/mode 2>/dev/null || true
'

MISTER_IP="$MISTER_IP" MISTER_PASS="$MISTER_PASS" scripts/mister ini-repair-boot
MISTER_IP="$MISTER_IP" MISTER_PASS="$MISTER_PASS" scripts/mister ini-repair-arcade-video

echo "==> Reboot to apply"
MISTER_IP="$MISTER_IP" MISTER_PASS="$MISTER_PASS" scripts/mister reboot-wait

echo "Done. Stock MiSTer should hand off to MiSTer_MagiK via MiSTer.ini main=."
