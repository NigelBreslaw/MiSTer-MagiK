#!/usr/bin/env bash
# Assemble the self-contained ARM bundle that gets copied to the MiSTer at
# /media/fat/mister-slint/.
#
# The bundle contains:
#   - python/           a portable CPython 3.12 for armv7 (python-build-standalone)
#   - python/.../site-packages/slint{,.libs}/   the Slint armv7l wheel, unpacked
#   - ui/, src/         the application
#   - fonts/, etc/fonts the bundled font + fontconfig config (MiSTer ships none)
#   - run-mister.sh     the on-device launcher
#
# Everything is downloaded once and cached under build/cache/. Re-running the
# script rebuilds build/mister-slint/ from those cached artifacts.
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BUILD="$HERE/build"
CACHE="$BUILD/cache"
BUNDLE="$BUILD/mister-slint"

PY_VER="3.12.12"
PY_TAG="20260127"
PY_FILE="cpython-${PY_VER}+${PY_TAG}-armv7-unknown-linux-gnueabihf-install_only_stripped.tar.gz"
PY_URL="https://github.com/astral-sh/python-build-standalone/releases/download/${PY_TAG}/${PY_FILE}"

WHEEL_FILE="slint-1.16.1b1-cp311-abi3-manylinux_2_31_armv7l.whl"
WHEEL_URL="https://files.pythonhosted.org/packages/f2/8f/ad1b39dbf5a6747980c9878522fbb7897eaaec8e64951d01a99ee1823834/${WHEEL_FILE}"

FONT_FILE="dejavu-fonts-ttf-2.37.zip"
FONT_URL="https://github.com/dejavu-fonts/dejavu-fonts/releases/download/version_2_37/${FONT_FILE}"

mkdir -p "$CACHE"
rm -rf "$BUNDLE"
mkdir -p "$BUNDLE"

fetch() { # url dest
    if [ ! -f "$2" ]; then
        echo "  downloading $(basename "$2")"
        curl -fL --retry 3 "$1" -o "$2"
    else
        echo "  cached $(basename "$2")"
    fi
}

echo "==> Fetching artifacts"
fetch "$PY_URL"    "$CACHE/$PY_FILE"
fetch "$WHEEL_URL" "$CACHE/$WHEEL_FILE"
fetch "$FONT_URL"  "$CACHE/$FONT_FILE"

echo "==> Unpacking portable CPython ${PY_VER}"
tar -xf "$CACHE/$PY_FILE" -C "$BUILD"      # extracts to $BUILD/python
rm -rf "$BUNDLE/python"
mv "$BUILD/python" "$BUNDLE/python"

echo "==> Installing Slint wheel into site-packages"
SP="$BUNDLE/python/lib/python3.12/site-packages"
mkdir -p "$SP"
( cd "$SP" && unzip -oq "$CACHE/$WHEEL_FILE" )

echo "==> Copying application"
cp -R "$HERE/ui"  "$BUNDLE/ui"
cp -R "$HERE/src" "$BUNDLE/src"
cp "$HERE/scripts/run-mister.sh" "$BUNDLE/run-mister.sh"
chmod +x "$BUNDLE/run-mister.sh"
# Drop caches that would otherwise bloat the upload.
find "$BUNDLE" -name '__pycache__' -type d -prune -exec rm -rf {} + 2>/dev/null || true

echo "==> Adding bundled font + fontconfig (MiSTer rootfs has neither)"
rm -rf "$BUILD/dejavu"
unzip -oq "$CACHE/$FONT_FILE" -d "$BUILD/dejavu"
mkdir -p "$BUNDLE/fonts" "$BUNDLE/etc/fonts"
find "$BUILD/dejavu" -name 'DejaVuSans.ttf'      -exec cp {} "$BUNDLE/fonts/" \;
find "$BUILD/dejavu" -name 'DejaVuSans-Bold.ttf' -exec cp {} "$BUNDLE/fonts/" \;

cat > "$BUNDLE/etc/fonts/fonts.conf" <<'CONF'
<?xml version="1.0"?>
<!-- Minimal fontconfig used on the MiSTer, which has no system fonts.
     FONTCONFIG_FILE points here; everything resolves to the bundled DejaVu. -->
<fontconfig>
  <dir>/media/fat/mister-slint/fonts</dir>
  <cachedir>/tmp/mister-slint/cache/fontconfig</cachedir>

  <!-- Map the generic families Slint asks for to the bundled font. -->
  <alias><family>sans-serif</family><prefer><family>DejaVu Sans</family></prefer></alias>
  <alias><family>serif</family><prefer><family>DejaVu Sans</family></prefer></alias>
  <alias><family>monospace</family><prefer><family>DejaVu Sans</family></prefer></alias>

  <!-- Last-resort: hand back DejaVu Sans for any unmatched request. -->
  <match target="pattern">
    <test name="family" qual="all"><string>DejaVu Sans</string></test>
  </match>
</fontconfig>
CONF

echo "==> Bundle ready"
du -sh "$BUNDLE"
echo "    python : $("$BUNDLE/python/bin/python3.12" --version 2>&1 || echo '(cannot exec on this host - that is expected for an ARM binary)')"
echo "    layout :"
( cd "$BUNDLE" && ls -1 )
echo
echo "Next: deploy with scripts/deploy-mister.sh (needs MISTER_IP / MISTER_PASS)."
