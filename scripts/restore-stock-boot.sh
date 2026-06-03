#!/usr/bin/env bash
# Restore stock MiSTer boot (inittab + optional MiSTer.ini from .bak).
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
MISTER_IP="${MISTER_IP:?Set MISTER_IP}"
MISTER_PASS="${MISTER_PASS:-1}"

MISTER_IP="$MISTER_IP" MISTER_PASS="$MISTER_PASS" uv run python scripts/mister_ssh.py run '
set -e
mount -o remount,rw / 2>/dev/null || true
sed -i "s|::sysinit:/media/fat/mister-magic/boot.sh &|::sysinit:/media/fat/MiSTer &|" /etc/inittab
if [ -f /media/fat/MiSTer.ini.bak ]; then
  cp /media/fat/MiSTer.ini.bak /media/fat/MiSTer.ini
  echo "restored MiSTer.ini from .bak"
fi
grep sysinit /etc/inittab | grep MiSTer
killall mister-magic-fb 2>/dev/null || true
'

MISTER_IP="$MISTER_IP" MISTER_PASS="$MISTER_PASS" uv run python scripts/mister_ssh.py reboot-wait
echo "Stock MiSTer boot restored."
