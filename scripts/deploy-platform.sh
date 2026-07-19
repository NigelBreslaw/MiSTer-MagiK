#!/usr/bin/env bash
# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

# Build and atomically deploy the complete MiSTer MagiK development platform.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
source "$ROOT/scripts/lib/magik-layout.sh"
source "$ROOT/scripts/lib/platform-manifest-lib.sh"
magik_layout_select dev
GUI_DIR="$ROOT/apps/mister"
MAIN_DIR="${MISTER_MAIN_DIR:-$ROOT/../Main_MiSTer}"
CLEAN_MAIN=0

usage() {
  cat <<'EOF'
Usage: scripts/deploy-platform.sh [--clean-main]

Builds Main and the ARM frontend, verifies the prebuilt qualified FPGA/module
artifacts, downloads and verifies the latest published game databases, uploads
every file with a temporary suffix, and activates manifests last. It never
writes /media/fat/menu.rbf and never reboots the device.
EOF
}

for arg in "$@"; do
  case "$arg" in
    --clean-main) CLEAN_MAIN=1 ;;
    -h|--help) usage; exit 0 ;;
    *) echo "ERROR: unknown argument: $arg" >&2; exit 2 ;;
  esac
done

GUI_BIN="$GUI_DIR/target/armv7-unknown-linux-gnueabihf/release-device/mister-magik-fb"
MAIN_BIN="$MAIN_DIR/bin/MiSTer"
MODULE="$ROOT/build/scanout-slots/mister_magik_scanout_slots.ko"
MODULE_META="$ROOT/build/scanout-slots/provenance.txt"
RBF="$ROOT/build/fpga-vblank-latch/menu-magik-vblank-latch.rbf"
RBF_META="$ROOT/build/fpga-vblank-latch/menu-magik-vblank-latch.metadata.txt"
MANIFEST="$ROOT/build/platform-v2.manifest"
REMOTE_APP="$MISTER_MAGIK_APP_DIR"
REMOTE_MAIN="$MISTER_MAGIK_MAIN"
DATABASE_STAGE="$(mktemp -d "${TMPDIR:-/tmp}/mister-magik-dev-databases.XXXXXX")"
rm -rf "$DATABASE_STAGE"
trap 'rm -rf "$DATABASE_STAGE"' EXIT

if [[ ! -d "$MAIN_DIR" || ! -x "$MAIN_DIR/build-container.sh" ]]; then
  echo "ERROR: Main_MiSTer fork checkout not found: $MAIN_DIR" >&2
  exit 1
fi
if [[ -n "$(git -C "$MAIN_DIR" status --porcelain --untracked-files=all)" ]]; then
  echo "WARN: deploying development Main from a dirty worktree" >&2
fi
if [[ -n "$(git -C "$ROOT" status --porcelain --untracked-files=no)" ]]; then
  echo "WARN: deploying development MagiK from a dirty worktree" >&2
fi
for artifact in "$MODULE" "$MODULE_META" "$RBF" "$RBF_META"; do
  if [[ ! -f "$artifact" ]]; then
    echo "ERROR: missing qualified artifact: $artifact" >&2
    exit 1
  fi
done
"$ROOT/scripts/checks/verify-fpga-rbf-manifest.py" "$RBF_META" >/dev/null

"$ROOT/scripts/fetch-game-databases-release.sh" "$DATABASE_STAGE"
MAME_DATABASE="$DATABASE_STAGE/mame.sqlite3"
HBMAME_DATABASE="$DATABASE_STAGE/hbmame.sqlite3"
DATABASE_MANIFEST="$DATABASE_STAGE/game-databases-manifest.json"
DATABASE_CHECKSUMS="$DATABASE_STAGE/SHA256SUMS"

echo "==> Building apps/mister development binary"
"$GUI_DIR/build-arm.sh" --device
echo "==> Building Main fork"
if [[ "$CLEAN_MAIN" == 1 ]]; then "$MAIN_DIR/build-container.sh" clean; fi
"$MAIN_DIR/build-container.sh"

MAIN_REVISION="$(git -C "$MAIN_DIR" rev-parse HEAD)"
MAGIK_REVISION="$(git -C "$ROOT" rev-parse HEAD)"
"$ROOT/scripts/release/platform/platform-manifest.py" generate \
  --output "$MANIFEST" \
  --main "$MAIN_BIN" \
  --gui "$GUI_BIN" \
  --scanout-module "$MODULE" \
  --scanout-metadata "$MODULE_META" \
  --latch-rbf "$RBF" \
  --latch-metadata "$RBF_META" \
  --main-revision "$MAIN_REVISION" \
  --magik-revision "$MAGIK_REVISION" \
  --layout dev >/dev/null

echo "==> Snapshotting and suspending the active launcher"
"$ROOT/scripts/mister" run '
set -e
STAMP=$(date +%Y%m%d-%H%M%S 2>/dev/null || echo unknown)
SNAP="/media/fat/mister-magik-dev/snapshots/$STAMP-deploy"
mkdir -p "$SNAP" /media/fat/mister-magik-dev/fpga
cp /etc/inittab "$SNAP/inittab" 2>/dev/null || true
cp /media/fat/MiSTer.ini "$SNAP/MiSTer.ini" 2>/dev/null || true
cp /media/fat/mister-magik-dev/platform-v2.manifest "$SNAP/platform-v2.manifest" 2>/dev/null || true
echo "snapshot: $SNAP"
'
"$ROOT/scripts/mister" agent magik suspend

declare -a LOCAL=(
  "$GUI_BIN" "$MAIN_BIN" "$MODULE" "$MODULE_META" "$RBF" "$RBF_META"
  "$MAME_DATABASE" "$HBMAME_DATABASE" "$DATABASE_MANIFEST" "$DATABASE_CHECKSUMS"
  "$MANIFEST"
)
declare -a REMOTE=(
  /media/fat/mister-magik-dev/mister-magik-fb
  /media/fat/MiSTer_MagiKDev
  /media/fat/mister-magik-dev/mister_magik_scanout_slots.ko
  /media/fat/mister-magik-dev/mister_magik_scanout_slots.metadata.txt
  /media/fat/mister-magik-dev/fpga/menu-magik-vblank-latch.rbf
  /media/fat/mister-magik-dev/fpga/menu-magik-vblank-latch.metadata.txt
  /media/fat/mister-magik-dev/mame.sqlite3
  /media/fat/mister-magik-dev/hbmame.sqlite3
  /media/fat/mister-magik-dev/game-databases-manifest.json
  /media/fat/mister-magik-dev/game-databases-SHA256SUMS
  /media/fat/mister-magik-dev/platform-v2.manifest
)
echo "==> Uploading inactive development bundle"
for index in "${!LOCAL[@]}"; do
  "$ROOT/scripts/mister" put "${LOCAL[$index]}" "${REMOTE[$index]}.upload"
done

echo "==> Verifying inactive bundle and activating manifest last"
platform_manifest_verify "$ROOT/scripts/mister" dev \
  /media/fat/mister-magik-dev/platform-v2.manifest.upload .upload verify
"$ROOT/scripts/mister" run '
set -e
manifest=/media/fat/mister-magik-dev/platform-v2.manifest.upload
while read -r expected name; do
  test -n "$expected" && test -n "$name"
  actual=$(sha256sum "/media/fat/mister-magik-dev/$name.upload" | awk "{print \$1}")
  test "$actual" = "$expected"
done < /media/fat/mister-magik-dev/game-databases-SHA256SUMS.upload
mv /media/fat/mister-magik-dev/mister-magik-fb.upload /media/fat/mister-magik-dev/mister-magik-fb
mv /media/fat/MiSTer_MagiKDev.upload /media/fat/MiSTer_MagiKDev
mv /media/fat/mister-magik-dev/mister_magik_scanout_slots.ko.upload /media/fat/mister-magik-dev/mister_magik_scanout_slots.ko
mv /media/fat/mister-magik-dev/mister_magik_scanout_slots.metadata.txt.upload /media/fat/mister-magik-dev/mister_magik_scanout_slots.metadata.txt
mv /media/fat/mister-magik-dev/fpga/menu-magik-vblank-latch.rbf.upload /media/fat/mister-magik-dev/fpga/menu-magik-vblank-latch.rbf
mv /media/fat/mister-magik-dev/fpga/menu-magik-vblank-latch.metadata.txt.upload /media/fat/mister-magik-dev/fpga/menu-magik-vblank-latch.metadata.txt
mv /media/fat/mister-magik-dev/mame.sqlite3.upload /media/fat/mister-magik-dev/mame.sqlite3
mv /media/fat/mister-magik-dev/hbmame.sqlite3.upload /media/fat/mister-magik-dev/hbmame.sqlite3
mv /media/fat/mister-magik-dev/game-databases-SHA256SUMS.upload /media/fat/mister-magik-dev/game-databases-SHA256SUMS
mv /media/fat/mister-magik-dev/game-databases-manifest.json.upload /media/fat/mister-magik-dev/game-databases-manifest.json
chmod 755 /media/fat/MiSTer_MagiKDev /media/fat/mister-magik-dev/mister-magik-fb
chmod 600 /media/fat/mister-magik-dev/mister_magik_scanout_slots.ko /media/fat/mister-magik-dev/mister_magik_scanout_slots.metadata.txt /media/fat/mister-magik-dev/fpga/menu-magik-vblank-latch.rbf /media/fat/mister-magik-dev/fpga/menu-magik-vblank-latch.metadata.txt /media/fat/mister-magik-dev/mame.sqlite3 /media/fat/mister-magik-dev/hbmame.sqlite3 /media/fat/mister-magik-dev/game-databases-manifest.json /media/fat/mister-magik-dev/game-databases-SHA256SUMS
sync
mv "$manifest" /media/fat/mister-magik-dev/platform-v2.manifest
sync
'

echo "Installed complete development platform bundle; /media/fat/menu.rbf and MiSTer.ini were not modified."
echo "Activate it with scripts/magik-mode.sh dev."
