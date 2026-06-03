#!/usr/bin/env bash
# Inspect a MiSTer over SSH — CPU, glibc, framebuffer, memory.
# Also verifies Cortex-A9 + NEON in /proc/cpuinfo (A1 toolchain prerequisite).
#
#   MISTER_IP=192.168.1.117 MISTER_PASS=1 scripts/audit-mister.sh
#
# Exits 1 if A1 CPU checks fail (unexpected part, missing neon/vfpv3, or not armv7l).
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
export MISTER_IP="${MISTER_IP:-192.168.1.117}"
export MISTER_PASS="${MISTER_PASS:-1}"

rc=0
uv run python "$HERE/scripts/mister_ssh.py" run '
echo "=== OS / CPU ==="
uname -a
uname -m

echo "=== CPU / A1 toolchain prerequisite ==="
FAIL=0
CPU_PART=$(awk -F: "/^CPU part/ { gsub(/ /,\"\",\$2); print \$2; exit }" /proc/cpuinfo)
FEATURES=$(awk -F: "/^Features/ { sub(/^ /,\"\",\$2); print \$2; exit }" /proc/cpuinfo)
HARDWARE=$(awk -F: "/^Hardware/ { sub(/^ /,\"\",\$2); print \$2; exit }" /proc/cpuinfo)
NCPU=$(grep -c "^processor" /proc/cpuinfo || echo 0)
echo "processors: $NCPU"
echo "Hardware: ${HARDWARE:-?}"
echo "CPU part: $CPU_PART (expect 0xc09 = ARM Cortex-A9)"
echo "Features: $FEATURES"

case "$CPU_PART" in
  0xc09) echo "Cortex-A9: yes" ;;
  *) echo "Cortex-A9: NO — unexpected CPU part"; FAIL=1 ;;
esac

case " $FEATURES " in
  *" neon "*) echo "NEON: yes" ;;
  *) echo "NEON: NO"; FAIL=1 ;;
esac

case " $FEATURES " in
  *" vfpv3 "*) echo "VFPv3: yes" ;;
  *) echo "VFPv3: NO (A1 rustflags use +vfp3)"; FAIL=1 ;;
esac

ARCH=$(uname -m)
echo "arch: $ARCH (expect armv7l)"
if [ "$ARCH" != armv7l ]; then
  echo "arch: unexpected for MiSTer HPS frontend"
  FAIL=1
fi

if [ "$FAIL" -ne 0 ]; then
  echo "A1 prerequisite: FAILED — do not enable +neon until cpuinfo matches"
  exit 1
fi
echo "A1 prerequisite: OK — safe for rustflags: target-cpu=cortex-a9, +neon,+vfp3"

echo "=== glibc ==="
ldd --version 2>&1 | head -1
echo "=== Framebuffer ==="
ls -l /dev/fb0 2>/dev/null
cat /sys/class/graphics/fb0/virtual_size 2>/dev/null
cat /sys/class/graphics/fb0/bits_per_pixel 2>/dev/null
cat /sys/class/graphics/fb0/stride 2>/dev/null
echo "=== DRM (expected: none) ==="
ls /dev/dri 2>/dev/null || echo "no /dev/dri"
echo "=== Memory ==="
grep MemTotal /proc/meminfo
echo "=== Rust binary ==="
ls -lh /media/fat/mister-magic/mister-magic-fb 2>/dev/null || echo "not deployed"
file /media/fat/mister-magic/mister-magic-fb 2>/dev/null || true
' || rc=$?

exit "$rc"
