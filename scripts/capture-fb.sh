#!/usr/bin/env bash
# Grab the MiSTer's framebuffer and save it as a PNG on this machine, so you
# can see what is actually on the HDMI output.
#
#   MISTER_IP=192.168.1.117 MISTER_PASS=1 scripts/capture-fb.sh [out.png]
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
IP="${MISTER_IP:-192.168.1.117}"
PASS="${MISTER_PASS:-1}"
OUT="${1:-$HERE/build/mister-fb.png}"
RAW="$HERE/build/mister-fb.raw"
W="${FB_W:-1920}"
H="${FB_H:-1080}"
mkdir -p "$HERE/build"

SSHOPTS="-o IdentitiesOnly=yes -o PubkeyAuthentication=no -o PreferredAuthentications=password,keyboard-interactive -o StrictHostKeyChecking=no -o ConnectTimeout=15"

expect <<EXP
set timeout 60
spawn ssh $SSHOPTS root@$IP {dd if=/dev/fb0 of=/tmp/fb0.raw bs=1M 2>/dev/null; echo CAP_DONE}
expect { -re {(?i)password:} { send "$PASS\r"; exp_continue } "CAP_DONE" {} timeout { exit 2 } }
expect eof
spawn scp $SSHOPTS root@$IP:/tmp/fb0.raw "$RAW"
expect { -re {(?i)password:} { send "$PASS\r" } timeout { exit 2 } }
expect eof
EXP

python3 "$HERE/scripts/raw_to_png.py" "$RAW" "$W" "$H" "$OUT"
echo "Captured MiSTer framebuffer -> $OUT"
