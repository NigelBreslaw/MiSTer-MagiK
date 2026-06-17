#!/usr/bin/env bash
# Install/remove a reversible MiSTer early wired-network boot experiment.
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
MISTER="$HERE/scripts/mister"
BACKUP="/media/fat/mister-magik/inittab.fastnet.bak"
MARKER="mister-magik-fastnet"
FASTNET_LINE="::sysinit:/sbin/dhcpcd -f /etc/dhcpcd.conf & # $MARKER"

usage() {
  cat <<EOF
Usage: scripts/mister-fastnet-boot.sh <install|remove|status>

Adds an early inittab dhcpcd launch after /bin/mount -a so the wired static IP
can come up before the normal rcS S40/S41 network services. remove restores the
saved /etc/inittab backup when available.
EOF
}

remote_install() {
  "$MISTER" run "
set -eu
mkdir -p /media/fat/mister-magik
if grep -q '$MARKER' /etc/inittab; then
  echo fastnet=already-installed
  grep '$MARKER' /etc/inittab
  exit 0
fi
if [ ! -f '$BACKUP' ]; then
  cp /etc/inittab '$BACKUP'
fi
mount -o remount,rw /
tmp=/tmp/inittab.fastnet.\$\$
awk '
  BEGIN { inserted = 0 }
  { print }
  !inserted && \$0 == \"::sysinit:/bin/mount -a\" {
    print \"$FASTNET_LINE\"
    inserted = 1
  }
  END { if (!inserted) exit 2 }
' /etc/inittab > \"\$tmp\"
cp \"\$tmp\" /etc/inittab
rm -f \"\$tmp\"
sync
mount -o remount,ro / || true
echo fastnet=installed
grep '$MARKER' /etc/inittab
"
}

remote_remove() {
  "$MISTER" run "
set -eu
mount -o remount,rw /
if [ -f '$BACKUP' ]; then
  cp '$BACKUP' /etc/inittab
  echo fastnet=restored-backup
else
  tmp=/tmp/inittab.fastnet-remove.\$\$
  grep -v '$MARKER' /etc/inittab > \"\$tmp\"
  cp \"\$tmp\" /etc/inittab
  rm -f \"\$tmp\"
  echo fastnet=removed-marker
fi
sync
mount -o remount,ro / || true
if grep -q '$MARKER' /etc/inittab; then
  echo fastnet=still-present
  grep '$MARKER' /etc/inittab
  exit 1
fi
"
}

remote_status() {
  "$MISTER" run "
if grep -q '$MARKER' /etc/inittab; then
  echo fastnet=installed
  grep '$MARKER' /etc/inittab
else
  echo fastnet=not-installed
fi
if [ -f '$BACKUP' ]; then
  echo backup=$BACKUP
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
