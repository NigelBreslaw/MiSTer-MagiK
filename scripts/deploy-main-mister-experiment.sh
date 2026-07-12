#!/usr/bin/env bash
# Build and deploy the Main-as-parent experiment without rebooting:
#   - magik-gui Slint binary -> /media/fat/mister-magik/mister-magik-fb
#   - external Main fork     -> /media/fat/MiSTer_MagiK
#   - boot config            -> stock inittab + MiSTer.ini main=MiSTer_MagiK
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
GUI_DIR="$ROOT/magik-gui"
MAIN_DIR="${MISTER_MAIN_DIR:-$ROOT/../Main_MiSTer}"
GUI_REMOTE="/media/fat/mister-magik/mister-magik-fb"
GUI_ART_REMOTE="/media/fat/mister-magik/art/arcade-cabinet-preview.rgba"
MAIN_REMOTE="/media/fat/MiSTer_MagiK"
GUI_PROFILE=release-device
GUI_BUILD_ARGS=(--device)
CLEAN_MAIN=0

usage() {
  sed -n '2,5p' "$0" | sed 's/^# \{0,1\}//'
  cat <<'EOF'

Options:
  --device       Build magik-gui with the release-device profile (default).
  --clean-main   Run make clean inside the Main fork container before building.
  -h, --help     Show this help.

Environment:
  MISTER_MAIN_DIR  Path to the external Main_MiSTer checkout.
                   Defaults to ../Main_MiSTer.
EOF
}

for arg in "$@"; do
  case "$arg" in
    --device)
      GUI_PROFILE=release-device
      GUI_BUILD_ARGS=(--device)
      ;;
    --clean-main)
      CLEAN_MAIN=1
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      echo "ERROR: unknown argument: $arg" >&2
      exit 2
      ;;
  esac
done

GUI_BIN="$GUI_DIR/target/armv7-unknown-linux-gnueabihf/$GUI_PROFILE/mister-magik-fb"
MAIN_BIN="$MAIN_DIR/bin/MiSTer"
PLUGIN_KO="$ROOT/build/scanout-slots/mister_magik_scanout_slots.ko"
PLUGIN_META="$ROOT/build/scanout-slots/provenance.txt"
LATCH_RBF="$ROOT/build/fpga-vblank-latch/menu-magik-vblank-latch.rbf"
LATCH_META="$ROOT/build/fpga-vblank-latch/menu-magik-vblank-latch.metadata.txt"
PLUGIN_REMOTE="/media/fat/mister-magik/mister_magik_scanout_slots.ko"
PLUGIN_META_REMOTE="/media/fat/mister-magik/mister_magik_scanout_slots.metadata.txt"
PINNED_KERNEL_REVISION="f0fb626acadd07f0718934826b143b6e4c9ce81c"
PINNED_KERNEL_CONFIG_SHA256="7137fafad42831de17a46755f1bfcd3b56b6ac5922c1981a515ccd4ce6158068"
PINNED_UAPI_SHA256="2d8dffc12b76c7346cbd6291ece440e824719ebb6b564192e3ba3f692eb8c5b9"
PINNED_SOURCE_SHA256="2bd6b3cc4bc4718cbe7db18f88e10c0f5a37585821a9c47075ea4c65adef92fc"
LATCH_RBF_REMOTE="/media/fat/mister-magik/experiments/menu-magik-vblank-latch.rbf"
LATCH_META_REMOTE="/media/fat/mister-magik/experiments/menu-magik-vblank-latch.metadata.txt"

if [[ ! -d "$MAIN_DIR" || ! -f "$MAIN_DIR/Makefile" ]]; then
  cat >&2 <<EOF
ERROR: Main_MiSTer fork checkout not found.

Expected: $MAIN_DIR

Create or clone the external fork at ../Main_MiSTer, or set:

  MISTER_MAIN_DIR=/path/to/Main_MiSTer $0
EOF
  exit 1
fi

if [[ ! -f "$PLUGIN_KO" || ! -f "$PLUGIN_META" ]]; then
  echo "ERROR: missing production scanout module or provenance" >&2
  echo "Build it with scripts/build-scanout-slots-module.sh before deploying production latch boot." >&2
  exit 1
fi
PLUGIN_SHA256="$(sha256sum "$PLUGIN_KO" | awk '{print $1}')"
if [[ "$(sed -n 's/^module_sha256=//p' "$PLUGIN_META")" != "$PLUGIN_SHA256" ]] ||
   ! grep -q '^vermagic=5\.15\.1-MiSTer ' "$PLUGIN_META" ||
   ! grep -q "^kernel_revision=$PINNED_KERNEL_REVISION$" "$PLUGIN_META" ||
   ! grep -q "^kernel_config_sha256=$PINNED_KERNEL_CONFIG_SHA256$" "$PLUGIN_META" ||
   ! grep -q "^uapi_sha256=$PINNED_UAPI_SHA256$" "$PLUGIN_META" ||
   ! grep -q "^source_sha256=$PINNED_SOURCE_SHA256$" "$PLUGIN_META" ||
   ! grep -q '^compiler=arm-linux-gnueabihf-gcc (Ubuntu 9\.4\.0' "$PLUGIN_META"; then
  echo "ERROR: scanout module does not match its provenance or target kernel" >&2
  exit 1
fi
if [[ ! -f "$LATCH_RBF" ]]; then
  echo "ERROR: missing CI-built latch RBF: $LATCH_RBF" >&2
  echo "Download it from the fpga-vblank-latch GitHub Actions workflow; do not build it locally." >&2
  exit 1
fi
"$ROOT/scripts/verify-fpga-rbf-manifest.py" "$LATCH_META"

echo "==> Building magik-gui ($GUI_PROFILE)"
"$GUI_DIR/build-arm.sh" "${GUI_BUILD_ARGS[@]}"

echo "==> Building Main fork: $MAIN_DIR"
if [[ ! -x "$MAIN_DIR/build-container.sh" ]]; then
  echo "ERROR: $MAIN_DIR/build-container.sh is missing or not executable." >&2
  exit 1
fi
if [[ "$CLEAN_MAIN" == 1 ]]; then
  "$MAIN_DIR/build-container.sh" clean
fi
"$MAIN_DIR/build-container.sh"

echo "==> Deploying experiment binaries"
"$ROOT/scripts/mister" run '
set -e
STAMP=$(date +%Y%m%d-%H%M%S 2>/dev/null || echo unknown)
SNAP="/media/fat/mister-magik/snapshots/$STAMP-deploy"
mkdir -p "$SNAP" /media/fat/mister-magik
cp /etc/inittab "$SNAP/inittab" 2>/dev/null || true
cp /media/fat/MiSTer.ini "$SNAP/MiSTer.ini" 2>/dev/null || true
ps > "$SNAP/ps.txt" 2>/dev/null || true
cat /sys/module/MiSTer_fb/parameters/mode > "$SNAP/fb-mode.txt" 2>/dev/null || true
cp /tmp/mister-magik-main.log "$SNAP/mister-magik-main.log" 2>/dev/null || true
echo "snapshot: $SNAP"
if [ -p /dev/MiSTer_cmd ] && pidof MiSTer_MagiK >/dev/null 2>&1; then
  printf "mister_magik_suspend\n" > /dev/MiSTer_cmd
  sleep 1
fi
'

"$ROOT/scripts/mister" put "$GUI_BIN" "$GUI_REMOTE.upload"
"$ROOT/scripts/mister" run "mkdir -p /media/fat/mister-magik/art"
GUI_ART_RAW="$(mktemp "${TMPDIR:-/tmp}/arcade-cabinet-preview.XXXXXX.rgba")"
python3 "$ROOT/scripts/png-to-slint-rgba.py" "$GUI_DIR/ui/art/arcade-cabinet-preview.png" "$GUI_ART_RAW"
"$ROOT/scripts/mister" put "$GUI_ART_RAW" "$GUI_ART_REMOTE.upload"
rm -f "$GUI_ART_RAW"

"$ROOT/scripts/mister" put "$MAIN_BIN" "$MAIN_REMOTE.upload"
"$ROOT/scripts/mister" run "mkdir -p /media/fat/mister-magik/experiments"
"$ROOT/scripts/mister" put "$PLUGIN_KO" "$PLUGIN_REMOTE.upload"
"$ROOT/scripts/mister" put "$PLUGIN_META" "$PLUGIN_META_REMOTE.upload"
"$ROOT/scripts/mister" put "$LATCH_RBF" "$LATCH_RBF_REMOTE.upload"
"$ROOT/scripts/mister" put "$LATCH_META" "$LATCH_META_REMOTE.upload"

"$ROOT/scripts/mister" run "expected=\$(sed -n 's/^rbf_sha256=//p' '$LATCH_META_REMOTE.upload'); actual=\$(sha256sum '$LATCH_RBF_REMOTE.upload' | awk '{print \$1}'); plugin_expected=\$(sed -n 's/^module_sha256=//p' '$PLUGIN_META_REMOTE.upload'); plugin_actual=\$(sha256sum '$PLUGIN_REMOTE.upload' | awk '{print \$1}'); test -n \"\$expected\"; test \"\$actual\" = \"\$expected\"; test \"\$plugin_expected\" = '$PLUGIN_SHA256'; test \"\$plugin_actual\" = '$PLUGIN_SHA256'; echo \"rbf_sha256=\$expected\" >> '$PLUGIN_META_REMOTE.upload'; mv '$GUI_REMOTE.upload' '$GUI_REMOTE'; mv '$GUI_ART_REMOTE.upload' '$GUI_ART_REMOTE'; mv '$MAIN_REMOTE.upload' '$MAIN_REMOTE'; mv '$PLUGIN_REMOTE.upload' '$PLUGIN_REMOTE'; mv '$PLUGIN_META_REMOTE.upload' '$PLUGIN_META_REMOTE'; mv '$LATCH_RBF_REMOTE.upload' '$LATCH_RBF_REMOTE'; mv '$LATCH_META_REMOTE.upload' '$LATCH_META_REMOTE'; chmod +x '$GUI_REMOTE' '$MAIN_REMOTE'; chmod 600 '$PLUGIN_REMOTE' '$PLUGIN_META_REMOTE' '$LATCH_RBF_REMOTE' '$LATCH_META_REMOTE'"

echo "==> Enabling stock inittab + MiSTer.ini main=MiSTer_MagiK boot"
"$ROOT/scripts/mister" run '
set -e
INI=/media/fat/MiSTer.ini
cp -n "$INI" "$INI.bak" 2>/dev/null || true

echo "=== post-install processes ==="
ps | grep -E "[M]iSTer|[M]iSTer_MagiK|[m]ister-magik-fb" || true
echo "=== post-install fb mode ==="
cat /sys/module/MiSTer_fb/parameters/mode 2>/dev/null || true
'

"$ROOT/scripts/mister" inittab-ensure-stock
"$ROOT/scripts/mister" ini-repair-boot
"$ROOT/scripts/mister" ini-repair-arcade-video

echo "==> Installed without rebooting."
echo "    Activate the deployed Main fork now with:"
echo "      scripts/mister run \"printf 'load_core menu.rbf\\n' > /dev/MiSTer_cmd\""
echo "    Or reboot later to let stock MiSTer hand off via MiSTer.ini main=MiSTer_MagiK."
echo "    Production latch boot files:"
echo "      $PLUGIN_REMOTE"
echo "      $LATCH_RBF_REMOTE"
echo "      $LATCH_META_REMOTE"
echo "    Restore stock with scripts/restore-stock-boot.sh."
