#!/usr/bin/env bash
# Inspect a MiSTer over SSH — CPU, glibc, framebuffer, memory.
#
#   MISTER_IP=192.168.1.117 MISTER_PASS=1 scripts/audit-mister.sh
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
export MISTER_IP="${MISTER_IP:-192.168.1.117}"
export MISTER_PASS="${MISTER_PASS:-1}"

uv run python "$HERE/scripts/mister_ssh.py" run '
echo "=== OS / CPU ==="; uname -a; uname -m
echo "=== glibc ==="; ldd --version 2>&1 | head -1
echo "=== Framebuffer ==="
ls -l /dev/fb0 2>/dev/null
cat /sys/class/graphics/fb0/virtual_size 2>/dev/null
cat /sys/class/graphics/fb0/bits_per_pixel 2>/dev/null
cat /sys/class/graphics/fb0/stride 2>/dev/null
echo "=== DRM (expected: none) ==="; ls /dev/dri 2>/dev/null || echo "no /dev/dri"
echo "=== Memory ==="; grep MemTotal /proc/meminfo
echo "=== Rust binary ==="
ls -lh /media/fat/mister-magic/mister-magic-fb 2>/dev/null || echo "not deployed"
file /media/fat/mister-magic/mister-magic-fb 2>/dev/null || true
'
