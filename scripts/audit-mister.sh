#!/usr/bin/env bash
# Inspect a MiSTer over SSH and report everything that matters for running the
# Slint bundle: CPU/OS, Python, glibc, the framebuffer, and the system
# libraries the Slint wheel needs from the device.
#
#   MISTER_IP=192.168.1.117 MISTER_PASS=1 scripts/audit-mister.sh
set -euo pipefail
IP="${MISTER_IP:-192.168.1.117}"
PASS="${MISTER_PASS:-1}"

REMOTE='
echo "=== OS / CPU ==="; uname -a; uname -m
echo "=== Python ==="; python3 --version 2>&1
echo "=== glibc ==="; ldd --version 2>&1 | head -1
echo "=== Framebuffer ==="; ls -l /dev/fb0 2>/dev/null; cat /sys/class/graphics/fb0/virtual_size 2>/dev/null; cat /sys/class/graphics/fb0/bits_per_pixel 2>/dev/null
echo "=== DRM (expected: none) ==="; ls /dev/dri 2>/dev/null || echo "no /dev/dri"
echo "=== System libs the wheel needs ==="
for s in libstdc++.so.6 libgcc_s.so.1 libglib-2.0.so.0 libgobject-2.0.so.0 libffi.so.7 libexpat.so.1 libz.so.1; do
  f=$(find /lib /usr/lib -maxdepth 2 -name "$s*" 2>/dev/null | head -1)
  [ -n "$f" ] && echo "PRESENT $s" || echo "MISSING $s"
done
echo "=== Fonts (expected: none) ==="; find /usr/share/fonts /media/fat -maxdepth 4 -name "*.ttf" 2>/dev/null | head -3 || true
echo "AUDIT_DONE"
'

exec expect <<EXP
set timeout 40
spawn ssh -o IdentitiesOnly=yes -o PubkeyAuthentication=no -o PreferredAuthentications=password,keyboard-interactive -o StrictHostKeyChecking=no -o ConnectTimeout=15 root@$IP {$REMOTE}
expect { -re {(?i)password:} { send "$PASS\r"; exp_continue } "AUDIT_DONE" {} timeout { puts "TIMEOUT"; exit 2 } eof {} }
expect eof
EXP
