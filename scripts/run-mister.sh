#!/bin/bash
# On-device launcher for the MiSTer Slint UI.
#
# This ships inside the bundle, so on the MiSTer it lives at
# /media/fat/mister-magic/run-mister.sh and is invoked by the Scripts-menu
# entry /media/fat/Scripts/mister-magic.sh.
set -u

APP="$(cd "$(dirname "$0")" && pwd)"
PYBIN="$APP/python/bin/python3.12"
LOG="/tmp/mister-magic.log"

mkdir -p /tmp/mister-magic/cache

# Render with the CPU (Skia software renderer) straight to the framebuffer.
# The MiSTer exposes a plain /dev/fb0 (no DRM/KMS), so force the legacy
# framebuffer path instead of letting the LinuxKMS backend probe /dev/dri.
export SLINT_BACKEND="linuxkms-skia-software"
export SLINT_BACKEND_LINUXFB="1"

# Optional performance instrumentation. Set MISTER_MAGIC_PERF=1 to make Slint
# print the achieved frame rate (and the active backend name) to the log and
# draw a live FPS counter over the UI. "refresh_lazy" reports the *real* rate
# driven by our animation; "refresh_full_speed" would busy-render AND disable
# partial rendering, which measures the worst case rather than what we ship.
# See AGENTS.md §9 (performance).
if [ "${MISTER_MAGIC_PERF:-0}" = "1" ]; then
    export SLINT_DEBUG_PERFORMANCE="refresh_lazy,console,overlay"
fi

# The MiSTer root filesystem ships no fonts and no fontconfig setup, so point
# fontconfig at the font that travels inside the bundle.
export FONTCONFIG_FILE="$APP/etc/fonts/fonts.conf"
export FONTCONFIG_PATH="$APP/etc/fonts"
export XDG_CACHE_HOME="/tmp/mister-magic/cache"
export HOME="/tmp/mister-magic"

# There is no ld.so cache on MiSTer, so spell out where to find the device's
# system libraries and the bundled Python's own libraries. Slint's vendored
# libraries (slint.libs) are found automatically via the extension's RPATH.
export LD_LIBRARY_PATH="$APP/python/lib:/usr/lib:/lib:${LD_LIBRARY_PATH:-}"

# The framebuffer is already 1920x1080x32 on a stock MiSTer, but set it
# explicitly so the UI is correct even if a core changed the video mode.
# Skip this when launched under a host that already configured the video
# pipeline and framebuffer (e.g. the Zaparoo boot path) — re-running vmode
# there can disturb the mode the menu core just set. Set MISTER_MAGIC_NO_VMODE=1.
if [ "${MISTER_MAGIC_NO_VMODE:-0}" != "1" ] && [ -x /usr/sbin/vmode ]; then
    /usr/sbin/vmode -r 1920 1080 rgb32 >/dev/null 2>&1 || true
fi

echo "[mister-magic] $(date) launching $PYBIN" >>"$LOG"
exec "$PYBIN" "$APP/src/main.py" >>"$LOG" 2>&1
