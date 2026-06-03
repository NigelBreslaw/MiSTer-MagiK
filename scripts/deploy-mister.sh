#!/usr/bin/env bash
# Deploy the built bundle to a MiSTer over SSH.
#
#   scripts/build-arm-bundle.sh      # build build/mister-magic/ first
#   MISTER_IP=192.168.1.117 MISTER_PASS=1 scripts/deploy-mister.sh
#
# Steps: pack the bundle into a tarball (dereferencing symlinks, since
# /media/fat is usually exFAT and cannot store them), copy it to the SD card,
# unpack it there, and drop the Scripts-menu entry in place.
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BUNDLE="$HERE/build/mister-magic"
TARBALL="$HERE/build/mister-magic.tar.gz"
ENTRY="$HERE/deploy/mister-magic.sh"
IP="${MISTER_IP:-192.168.1.117}"
PASS="${MISTER_PASS:-1}"

[ -d "$BUNDLE" ] || { echo "No bundle at $BUNDLE - run scripts/build-arm-bundle.sh first."; exit 1; }

echo "==> Packing bundle (dereferencing symlinks for exFAT/FAT)"
tar -czhf "$TARBALL" -C "$HERE/build" mister-magic
ls -lh "$TARBALL"

SSHOPTS="-o IdentitiesOnly=yes -o PubkeyAuthentication=no -o PreferredAuthentications=password,keyboard-interactive -o StrictHostKeyChecking=no -o ConnectTimeout=20"

echo "==> Uploading and installing on $IP"
expect <<EXP
set timeout 900
spawn scp $SSHOPTS "$TARBALL" root@$IP:/media/fat/mister-magic.tar.gz
expect { -re {(?i)password:} { send "$PASS\r" } timeout { puts "scp tarball TIMEOUT"; exit 2 } }
expect eof

spawn scp $SSHOPTS "$ENTRY" root@$IP:/media/fat/Scripts/mister-magic.sh
expect { -re {(?i)password:} { send "$PASS\r" } timeout { puts "scp entry TIMEOUT"; exit 2 } }
expect eof

spawn ssh $SSHOPTS root@$IP {cd /media/fat && rm -rf mister-magic && gzip -dc mister-magic.tar.gz | tar xf - && rm -f mister-magic.tar.gz && chmod -R a+rx mister-magic 2>/dev/null; echo DEPLOY_OK}
expect { -re {(?i)password:} { send "$PASS\r"; exp_continue } "DEPLOY_OK" {} timeout { puts "extract TIMEOUT"; exit 2 } }
expect eof
EXP

echo "==> Deployed. Launch from the MiSTer OSD: Scripts -> mister-magic"
echo "    Log on device: /tmp/mister-magic.log"
