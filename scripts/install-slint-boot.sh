#!/usr/bin/env bash
# Install Slint boot: revert main= override, deploy boot.sh, point inittab at it.
# MiSTer must run video_init (HDMI timing) before our binary — main= on our binary skips that.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

MISTER_IP="${MISTER_IP:?Set MISTER_IP}"
MISTER_PASS="${MISTER_PASS:-1}"

echo "==> Deploy boot.sh"
MISTER_IP="$MISTER_IP" MISTER_PASS="$MISTER_PASS" uv run python scripts/mister_ssh.py put \
  scripts/mister-magic/boot.sh /media/fat/mister-magic/boot.sh

echo "==> Configure device (revert main=, inittab, chmod)"
MISTER_IP="$MISTER_IP" MISTER_PASS="$MISTER_PASS" uv run python scripts/mister_ssh.py run '
set -e
chmod +x /media/fat/mister-magic/boot.sh

INI=/media/fat/MiSTer.ini
if [ ! -f "$INI.bak" ]; then cp "$INI" "$INI.bak"; fi

# Remove main=mister-magic — stock MiSTer must run video_init on the handoff pass.
tmp="$INI.new"
replaced=0
while IFS= read -r line || [ -n "$line" ]; do
  case "$line" in
    main=mister-magic/*)
      if [ "$replaced" = 0 ]; then
        echo ";main=mister-magic/mister-magic-fb  ; use boot.sh instead"
        replaced=1
      fi
      ;;
    *)
      echo "$line"
      ;;
  esac
done < "$INI" > "$tmp"
mv "$tmp" "$INI"
echo "main= override removed (MiSTer runs video_init during boot.sh handoff)"

# Point sysinit at boot.sh (was /media/fat/MiSTer &).
mount -o remount,rw / 2>/dev/null || true
if grep -q "mister-magic/boot.sh" /etc/inittab; then
  echo "inittab already uses boot.sh"
else
  sed -i "s|::sysinit:/media/fat/MiSTer &|::sysinit:/media/fat/mister-magic/boot.sh &|" /etc/inittab
  echo "inittab updated -> mister-magic/boot.sh"
fi
grep sysinit /etc/inittab | grep -E "MiSTer|boot.sh"
'

echo "==> Reboot to apply"
MISTER_IP="$MISTER_IP" MISTER_PASS="$MISTER_PASS" uv run python scripts/mister_ssh.py reboot-wait

echo "Done. TV should show Slint launcher after ~5-10s boot (brief stock menu flash possible)."
