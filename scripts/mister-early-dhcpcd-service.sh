#!/usr/bin/env bash
# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

# Install/remove a reversible S11 dhcpcd boot-order experiment on MiSTer.
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
MISTER="$HERE/scripts/mister"
EARLY_LINK="/etc/init.d/S11dhcpcd"
MARKER="/media/fat/mister-magik-dev/S11dhcpcd.experiment"

usage() {
  cat <<EOF
Usage: scripts/mister-early-dhcpcd-service.sh <install|remove|status>

Adds /etc/init.d/S11dhcpcd as a symlink to S41dhcpcd so dhcpcd starts right
after S10udev and before dbus/network/ntp/proftpd/sshd. remove deletes only
that experiment symlink.
EOF
}

remote_install() {
  "$MISTER" run "
set -eu
mkdir -p /media/fat/mister-magik-dev
mount -o remount,rw /
if [ -e '$EARLY_LINK' ] && [ ! -L '$EARLY_LINK' ]; then
  echo early_dhcpcd=blocked_existing_file
  ls -l '$EARLY_LINK'
  mount -o remount,ro / || true
  exit 1
fi
ln -sf S41dhcpcd '$EARLY_LINK'
echo installed >'$MARKER'
sync
mount -o remount,ro / || true
echo early_dhcpcd=installed
ls -l '$EARLY_LINK'
"
}

remote_remove() {
  "$MISTER" run "
set -eu
mount -o remount,rw /
if [ -L '$EARLY_LINK' ]; then
  rm -f '$EARLY_LINK'
  echo early_dhcpcd=removed
else
  echo early_dhcpcd=not-installed
fi
rm -f '$MARKER'
sync
mount -o remount,ro / || true
"
}

remote_status() {
  "$MISTER" run "
if [ -L '$EARLY_LINK' ]; then
  echo early_dhcpcd=installed
  ls -l '$EARLY_LINK'
else
  echo early_dhcpcd=not-installed
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
