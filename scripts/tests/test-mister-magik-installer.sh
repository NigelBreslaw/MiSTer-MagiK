#!/usr/bin/env bash
# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT
FAT="$TMP/fat"
APP="$FAT/mister-magik"
mkdir -p "$APP/fpga" "$FAT/Scripts"

printf '#!/bin/sh\n' >"$FAT/MiSTer_MagiK"
printf '#!/bin/sh\n' >"$APP/mister-magik-fb"
cp "$ROOT/mister/tools/manager/target/debug/mister-magik-manager" "$APP/mister-magik-manager"
chmod 755 "$APP/mister-magik-manager"
printf 'module\n' >"$APP/mister_magik_scanout_slots.ko"
printf 'rbf\n' >"$APP/fpga/menu-magik-vblank-latch.rbf"
contract="$(printf contract | sha256sum | awk '{print $1}')"
module_hash="$(sha256sum "$APP/mister_magik_scanout_slots.ko" | awk '{print $1}')"
rbf_hash="$(sha256sum "$APP/fpga/menu-magik-vblank-latch.rbf" | awk '{print $1}')"
printf 'platform_contract_sha256=%s\nmodule_sha256=%s\nvermagic=5.15.1-MiSTer fixture\n' \
  "$contract" "$module_hash" >"$APP/mister_magik_scanout_slots.metadata.txt"
printf 'platform_contract_sha256=%s\nsource_commit=%040d\nlatch_protocol_version=5\nlatch_capability_mask=0x03ff\nrbf_sha256=%s\n' \
  "$contract" 3 "$rbf_hash" >"$APP/fpga/menu-magik-vblank-latch.metadata.txt"
printf '{"format":"mister-magik-platform-bundle-v0.2","release_version":16,"bundle_id":"%064d"}\n' 0 \
  >"$APP/platform-bundle-v0.2.json"
"$ROOT/scripts/agent" ci platform-manifest generate \
  --output "$APP/platform-v3.manifest" --layout public \
  --main "$FAT/MiSTer_MagiK" --gui "$APP/mister-magik-fb" \
  --manager "$APP/mister-magik-manager" \
  --scanout-module "$APP/mister_magik_scanout_slots.ko" \
  --scanout-metadata "$APP/mister_magik_scanout_slots.metadata.txt" \
  --latch-rbf "$APP/fpga/menu-magik-vblank-latch.rbf" \
  --latch-metadata "$APP/fpga/menu-magik-vblank-latch.metadata.txt" \
  --platform-bundle-manifest "$APP/platform-bundle-v0.2.json" \
  --main-revision "$(printf %040d 2)" --magik-revision "$(printf %040d 1)" >/dev/null

cp "$ROOT/scripts/MiSTer-MagiK.sh" "$FAT/Scripts/MiSTer-MagiK.sh"
cp "$ROOT/mister/platform/contracts/generated/platform-v3.constants.sh" \
  "$FAT/Scripts/MiSTer-MagiK.platform-v3.constants.sh"
chmod 755 "$FAT/Scripts/MiSTer-MagiK.sh"
printf '::sysinit:/media/fat/MiSTer &\n' >"$TMP/inittab"
printf '[MiSTer]\r\nmain=MiSTer\r\nmain=Other ; retain\r\n[Menu]\r\ndirect_video=9\r\ndirect_video=8 ; retain\r\nmenu_pal=9\r\nforced_scandoubler=9\r\nvideo_mode=8\r\nuser=keep\r\n' >"$FAT/MiSTer.ini"
cp "$FAT/MiSTer.ini" "$TMP/original.ini"

run_manager() {
  MISTER_MAGIK_FAT="$FAT" MISTER_MAGIK_INITTAB="$TMP/inittab" \
    MISTER_MAGIK_TEST_MODE=1 \
    MISTER_MAGIK_TEST_KEYS="${MISTER_MAGIK_TEST_KEYS:-}" \
    "$FAT/Scripts/MiSTer-MagiK.sh" "$@"
}

# A/Enter is not a safety confirmation and cannot mutate either boot file.
before_ini="$(sha256sum "$FAT/MiSTer.ini")"
before_inittab="$(sha256sum "$TMP/inittab")"
if MISTER_MAGIK_TEST_CONFIRM_INSTALL=1 MISTER_MAGIK_TEST_KEYS=enter run_manager install >"$TMP/enter.log" 2>&1; then
  echo "Enter unexpectedly confirmed installation" >&2
  exit 1
fi
grep -q 'Press Down on the keyboard or joystick' "$TMP/enter.log"
test "$(sha256sum "$FAT/MiSTer.ini")" = "$before_ini"
test "$(sha256sum "$TMP/inittab")" = "$before_inittab"

# Malformed, duplicate, and noncanonical bootstrap fields are refused byte-identically.
cp "$APP/platform-v3.manifest" "$TMP/manifest.good"
for case_name in malformed duplicate noncanonical; do
  cp "$TMP/manifest.good" "$APP/platform-v3.manifest"
  case "$case_name" in
    malformed)
      sed -i.bak 's/^manager_sha256=.*/manager_sha256=UPPERCASE/' "$APP/platform-v3.manifest"
      ;;
    duplicate)
      printf 'manager_path=/media/fat/mister-magik/mister-magik-manager\n' >>"$APP/platform-v3.manifest"
      ;;
    noncanonical)
      sed -i.bak 's#^manager_path=.*#manager_path=/media/fat/mister-magik/../mister-magik/mister-magik-manager#' "$APP/platform-v3.manifest"
      ;;
  esac
  if MISTER_MAGIK_TEST_KEYS=down run_manager install >"$TMP/bootstrap-$case_name.log" 2>&1; then
    echo "$case_name manifest unexpectedly ran" >&2
    exit 1
  fi
  test "$(sha256sum "$FAT/MiSTer.ini")" = "$before_ini"
  test "$(sha256sum "$TMP/inittab")" = "$before_inittab"
done
cp "$TMP/manifest.good" "$APP/platform-v3.manifest"

# The bootstrap reports a missing hashing tool and refuses to run the manager.
mkdir "$TMP/bootstrap-bin"
for tool in grep sed awk chmod env; do
  ln -s "$(command -v "$tool")" "$TMP/bootstrap-bin/$tool"
done
if PATH="$TMP/bootstrap-bin" MISTER_MAGIK_TEST_KEYS=down run_manager install >"$TMP/missing-tool.log" 2>&1; then
  echo "bootstrap unexpectedly ran without sha256sum" >&2
  exit 1
fi
grep -q 'required tool is unavailable: sha256sum' "$TMP/missing-tool.log"
test "$(sha256sum "$FAT/MiSTer.ini")" = "$before_ini"
test "$(sha256sum "$TMP/inittab")" = "$before_inittab"

# A missing manager is refused by the bootstrap without touching boot files.
mv "$APP/mister-magik-manager" "$TMP/manager.missing"
if MISTER_MAGIK_TEST_KEYS=down run_manager install >"$TMP/missing.log" 2>&1; then
  echo "missing manager unexpectedly ran" >&2
  exit 1
fi
grep -q 'missing .*mister-magik-manager' "$TMP/missing.log"
test "$(sha256sum "$FAT/MiSTer.ini")" = "$before_ini"
mv "$TMP/manager.missing" "$APP/mister-magik-manager"

# A corrupt manager is rejected by the shell before it can run.
cp "$APP/mister-magik-manager" "$TMP/manager.good"
printf 'corrupt\n' >>"$APP/mister-magik-manager"
if MISTER_MAGIK_TEST_KEYS=down run_manager install >"$TMP/corrupt.log" 2>&1; then
  echo "corrupt manager unexpectedly ran" >&2
  exit 1
fi
grep -q 'manager hash mismatch' "$TMP/corrupt.log"
test "$(sha256sum "$FAT/MiSTer.ini")" = "$before_ini"
cp "$TMP/manager.good" "$APP/mister-magik-manager"
chmod 755 "$APP/mister-magik-manager"

# Keyboard/joystick Down confirms installation and the Main duplicate is canonicalized.
MISTER_MAGIK_TEST_KEYS=down run_manager install >"$TMP/install.log"
grep -q 'installed. Rebooting to start MiSTer MagiK' "$TMP/install.log"
grep -q 'TEST: normal reboot requested' "$TMP/install.log"
if grep -q 'Choose launcher output' "$TMP/install.log"; then
  echo "installer unexpectedly prompted for an output mode" >&2
  exit 1
fi
cmp "$TMP/original.ini" "$FAT/MiSTer.ini.bak.before-magik"
python3 - "$FAT/MiSTer.ini" <<'PY'
import pathlib, sys
data = pathlib.Path(sys.argv[1]).read_bytes()
original = pathlib.Path(str(sys.argv[1]) + ".bak.before-magik").read_bytes()
assert b"\n" not in data.replace(b"\r\n", b"")
text = data.decode()
assert text.count("main=MiSTer_MagiK") == 1
assert text.count("direct_video=9") == 1
assert text.count("direct_video=8") == 1
assert text.count("menu_pal=9") == 1
assert text.count("forced_scandoubler=9") == 1
assert text.count("video_mode=8") == 1
assert ";main=Other ; retain" in text
assert "direct_video=8 ; retain" in text
assert "user=keep" in text
assert data[data.index(b"[Menu]"):] == original[original.index(b"[Menu]"):]
PY

# Restore changes only owned keys and retains post-install user changes.
printf 'after_install=keep\r\n' >>"$FAT/MiSTer.ini"
run_manager restore >"$TMP/restore.log"
grep -q 'stock MiSTer boot restored' "$TMP/restore.log"
grep -q 'after_install=keep' "$FAT/MiSTer.ini"
grep -q 'main=Other' "$FAT/MiSTer.ini"
grep -qx '::sysinit:/media/fat/MiSTer &' "$TMP/inittab"

# Full uninstall also requires Down and restores stock before deleting its manager.
MISTER_MAGIK_TEST_KEYS=down run_manager install >/dev/null
mkdir -p "$FAT/licenses" "$FAT/mister-magik-dev" "$FAT/Scripts/.mister-magik-user"
printf 'stock\n' >"$FAT/MiSTer"
printf 'stock menu\n' >"$FAT/menu.rbf"
printf 'user backup\n' >"$FAT/MiSTer.ini.bak"
printf 'developer\n' >"$FAT/MiSTer_MagiKDev"
printf 'developer payload\n' >"$FAT/mister-magik-dev/keep.txt"
printf 'user license\n' >"$FAT/licenses/USER-LICENSE.txt"
printf 'user hook\n' >"$FAT/Scripts/.mister-magik-user/hook"
for owned in \
  THIRD-PARTY-NOTICES.txt SOURCE-OFFER.txt \
  licenses/MiSTer-MagiK-GPL-3.0-or-later.txt \
  licenses/RUST-LIBRARIES.txt licenses/FFMPEG-LGPL-2.1-or-later.txt \
  licenses/PRESS-START-2P-OFL-1.1.txt \
  licenses/ARCADE-CABINET-CC-BY-NC-4.0.txt \
  downloader_mister_magik.ini \
  .downloader_mister_magik.ini.partial downloader_mister_magik.ini.tmp.123 \
  .MiSTer.ini.bak.before-magik.new.123 .MiSTer.ini.magik.new.123; do
  mkdir -p "$(dirname "$FAT/$owned")"
  printf 'owned\n' >"$FAT/$owned"
done
if MISTER_MAGIK_TEST_KEYS=enter run_manager uninstall >"$TMP/uninstall-enter.log" 2>&1; then
  echo "Enter unexpectedly confirmed uninstall" >&2
  exit 1
fi
test -d "$APP"
MISTER_MAGIK_TEST_KEYS=down run_manager uninstall >"$TMP/uninstall.log"
grep -q 'fully uninstalled' "$TMP/uninstall.log"
test ! -e "$APP"
test ! -e "$FAT/Scripts/MiSTer-MagiK.sh"
test ! -e "$FAT/Scripts/MiSTer-MagiK.platform-v3.constants.sh"
for owned in \
  THIRD-PARTY-NOTICES.txt SOURCE-OFFER.txt \
  licenses/MiSTer-MagiK-GPL-3.0-or-later.txt \
  licenses/RUST-LIBRARIES.txt licenses/FFMPEG-LGPL-2.1-or-later.txt \
  licenses/PRESS-START-2P-OFL-1.1.txt \
  licenses/ARCADE-CABINET-CC-BY-NC-4.0.txt \
  downloader_mister_magik.ini \
  .downloader_mister_magik.ini.partial downloader_mister_magik.ini.tmp.123 \
  .MiSTer.ini.bak.before-magik.new.123 .MiSTer.ini.magik.new.123; do
  test ! -e "$FAT/$owned"
done
test -e "$FAT/MiSTer"
test -e "$FAT/menu.rbf"
test -e "$FAT/MiSTer.ini.bak"
test -e "$FAT/MiSTer_MagiKDev"
test -e "$FAT/mister-magik-dev/keep.txt"
test -e "$FAT/licenses/USER-LICENSE.txt"
test -e "$FAT/Scripts/.mister-magik-user/hook"
grep -qx '::sysinit:/media/fat/MiSTer &' "$TMP/inittab"
grep -q 'main=Other' "$FAT/MiSTer.ini"
