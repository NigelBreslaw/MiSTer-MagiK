#!/usr/bin/env bash
# Install Magik boot through the Main-as-parent fork.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

MISTER_IP="${MISTER_IP:?Set MISTER_IP}"
MISTER_PASS="${MISTER_PASS:-1}"

echo "==> Configure device (direct MiSTer_Magik boot)"
MISTER_IP="$MISTER_IP" MISTER_PASS="$MISTER_PASS" uv run python scripts/mister_ssh.py run '
set -e
if [ ! -x /media/fat/MiSTer_Magik ]; then
  echo "ERROR: /media/fat/MiSTer_Magik is missing or not executable"
  echo "Run scripts/deploy-main-mister-experiment.sh first."
  exit 1
fi

INI=/media/fat/MiSTer.ini
if [ ! -f "$INI.before-mister-magik-main" ]; then cp "$INI" "$INI.before-mister-magik-main"; fi

# Remove main= overrides. MiSTer_Magik is started directly by inittab.
tmp="$INI.new"
while IFS= read -r line || [ -n "$line" ]; do
  case "$line" in
    main=*) ;;
    *)
      echo "$line"
      ;;
  esac
done < "$INI" > "$tmp"
mv "$tmp" "$INI"
echo "main= override removed"

# Point sysinit at MiSTer_Magik.
mount -o remount,rw / 2>/dev/null || true
if grep -q "^::sysinit:/media/fat/MiSTer_Magik " /etc/inittab; then
  echo "inittab already uses MiSTer_Magik"
else
  sed -i "s|^::sysinit:/media/fat/MiSTer &|::sysinit:/media/fat/MiSTer_Magik \&|" /etc/inittab
  sed -i "s|^::sysinit:/media/fat/mister-magic/boot.sh .*|::sysinit:/media/fat/MiSTer_Magik \&|" /etc/inittab
  echo "inittab updated -> MiSTer_Magik"
fi
grep sysinit /etc/inittab | grep -E "MiSTer|boot.sh"
'

echo "==> Reboot to apply"
MISTER_IP="$MISTER_IP" MISTER_PASS="$MISTER_PASS" uv run python scripts/mister_ssh.py reboot-wait

echo "Done. TV should show Magik after MiSTer_Magik initializes Main."
