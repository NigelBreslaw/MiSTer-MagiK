#!/usr/bin/env bash
# Install update_all-compatible Magik boot through MiSTer's native main= hook.
#
# Stock /media/fat/MiSTer stays as the only inittab menu entry. It reads
# MiSTer.ini and re-execs /media/fat/MiSTer_MagiK.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
source "$ROOT/scripts/reboot-wait-lib.sh"

echo "==> Configure device (stock inittab + MiSTer.ini main=MiSTer_MagiK)"
scripts/mister run '
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

echo "=== post-install processes ==="
ps | grep -E "[M]iSTer|[M]iSTer_MagiK|[m]ister-magik-fb" || true
echo "=== post-install fb mode ==="
cat /sys/module/MiSTer_fb/parameters/mode 2>/dev/null || true
'

scripts/mister inittab-ensure-stock
scripts/mister ini-repair-boot
scripts/mister ini-repair-arcade-video

mister_reboot_wait_with_raw_fallback

echo "Done. Stock MiSTer should hand off to MiSTer_MagiK via MiSTer.ini main=."
