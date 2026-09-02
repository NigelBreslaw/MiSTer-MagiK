#!/usr/bin/env bash
# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

MANAGER_BINARY="$ROOT/mister/tools/manager/target/debug/mister-magik-manager"
[ -x "$MANAGER_BINARY" ] || {
  echo "missing manager binary: $MANAGER_BINARY (run cargo build first)" >&2
  exit 1
}

new_fixture() {
  local name="$1"
  FIXTURE="$TMP/$name"
  FAT="$FIXTURE/fat"
  APP="$FAT/mister-magik"
  INITTAB="$FIXTURE/inittab"
  mkdir -p "$APP/fpga" "$FAT/Scripts"
  printf '#!/bin/sh\n' >"$FAT/MiSTer_MagiK"
  printf '::sysinit:/media/fat/MiSTer &\n' >"$INITTAB"
  printf '[MiSTer]\r\nmain=MiSTer\r\nmain=Other ; retain\r\n[Menu]\r\ndirect_video=9\r\ndirect_video=8 ; retain\r\nmenu_pal=9\r\nforced_scandoubler=9\r\nvideo_mode=8\r\nuser=keep\r\n' >"$FAT/MiSTer.ini"
  cp "$FAT/MiSTer.ini" "$FIXTURE/original.ini"
}

seed_package() {
  mkdir -p "$APP/fpga" "$FAT/Scripts"
  printf '#!/bin/sh\n' >"$FAT/MiSTer_MagiK"
  printf '#!/bin/sh\n' >"$APP/mister-magik-fb"
  cp "$MANAGER_BINARY" "$APP/mister-magik-manager"
  chmod 755 "$APP/mister-magik-manager"
  printf 'module\n' >"$APP/mister_magik_scanout_slots.ko"
  printf 'rbf\n' >"$APP/fpga/menu-magik-vblank-latch.rbf"
  local contract module_hash rbf_hash
  contract="$(printf contract | sha256sum | awk '{print $1}')"
  module_hash="$(sha256sum "$APP/mister_magik_scanout_slots.ko" | awk '{print $1}')"
  rbf_hash="$(sha256sum "$APP/fpga/menu-magik-vblank-latch.rbf" | awk '{print $1}')"
  printf 'platform_contract_sha256=%s\nmodule_sha256=%s\nvermagic=5.15.1-MiSTer fixture\n' \
    "$contract" "$module_hash" >"$APP/mister_magik_scanout_slots.metadata.txt"
  printf 'platform_contract_sha256=%s\nsource_commit=%040d\nlatch_protocol_version=5\nlatch_capability_mask=0x03ff\nsource_status= M menu.qsf\nsource_status= M sys/sys_top.sdc\nrbf_sha256=%s\n' \
    "$contract" 3 "$rbf_hash" >"$APP/fpga/menu-magik-vblank-latch.metadata.txt"
  printf '{"format":"mister-magik-platform-bundle-v0.2","release_version":16,"bundle_id":"%064d"}\n' 0 \
    >"$APP/platform-bundle-v0.2.json"
  "$ROOT/scripts/magik-ci" ci platform-manifest generate \
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
  chmod 755 "$FAT/Scripts/MiSTer-MagiK.sh"
}

run_manager() {
  MISTER_MAGIK_FAT="$FAT" MISTER_MAGIK_INITTAB="$INITTAB" \
    MISTER_MAGIK_TEST_MODE=1 \
    MISTER_MAGIK_TEST_KEYS="${MISTER_MAGIK_TEST_KEYS:-}" \
    MISTER_MAGIK_DOWNLOADER="${MISTER_MAGIK_DOWNLOADER:-}" \
    "$FAT/Scripts/MiSTer-MagiK.sh" "$@"
}

assert_boot_unchanged() {
  test "$(sha256sum "$FAT/MiSTer.ini")" = "$BEFORE_INI"
  test "$(sha256sum "$INITTAB")" = "$BEFORE_INITTAB"
}

assert_stock() {
  python3 - "$FAT/MiSTer.ini" <<'PY'
import pathlib
import sys

text = pathlib.Path(sys.argv[1]).read_text()
active = [line.strip() for line in text.splitlines() if line.strip().startswith("main=")]
assert len(active) == 1
assert active[0] != "main=MiSTer_MagiK"
PY
  grep -qx '::sysinit:/media/fat/MiSTer &' "$INITTAB"
}

assert_magik_selected() {
  python3 - "$FAT/MiSTer.ini" <<'PY'
import pathlib
import sys

text = pathlib.Path(sys.argv[1]).read_text()
active = [line.strip() for line in text.splitlines() if line.strip().startswith("main=")]
assert active == ["main=MiSTer_MagiK"]
PY
  grep -qx '::sysinit:/media/fat/MiSTer &' "$INITTAB"
}

assert_owned_files_removed() {
  local owned
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
  test ! -e "$FAT/MiSTer_MagiK"
  test ! -e "$FAT/MiSTer.ini.bak.before-magik"
  test ! -e "$APP"
  test ! -e "$FAT/Scripts/MiSTer-MagiK.sh"
  test ! -e "$FAT/Scripts/MiSTer-MagiK.platform-v3.constants.sh"
}

assert_unowned_files_preserved() {
  test -e "$FAT/MiSTer"
  test -e "$FAT/menu.rbf"
  test -e "$FAT/MiSTer.ini.bak"
  test -e "$FAT/MiSTer_MagiKDev"
  test -e "$FAT/mister-magik-dev/keep.txt"
  test -e "$FAT/licenses/USER-LICENSE.txt"
  test -e "$FAT/Scripts/.mister-magik-user/hook"
}

# A/Enter is not a safety confirmation and cannot mutate either boot file.
new_fixture cancel-install
seed_package
BEFORE_INI="$(sha256sum "$FAT/MiSTer.ini")"
BEFORE_INITTAB="$(sha256sum "$INITTAB")"
if MISTER_MAGIK_TEST_KEYS=enter run_manager >"$FIXTURE/cancel.log" 2>&1; then
  echo "Enter unexpectedly confirmed installation" >&2
  exit 1
fi
grep -q 'Press Down on the keyboard or joystick' "$FIXTURE/cancel.log"
assert_boot_unchanged

# The bootstrap reports a missing hashing tool and refuses to run the manager.
mkdir "$FIXTURE/bootstrap-bin"
for tool in dirname grep sed awk chmod env; do
  ln -s "$(command -v "$tool")" "$FIXTURE/bootstrap-bin/$tool"
done
if PATH="$FIXTURE/bootstrap-bin" MISTER_MAGIK_TEST_KEYS=down run_manager >"$FIXTURE/missing-tool.log" 2>&1; then
  echo "bootstrap unexpectedly ran without sha256sum" >&2
  exit 1
fi
grep -q 'required tool is unavailable: sha256sum' "$FIXTURE/missing-tool.log"
assert_boot_unchanged

# Malformed, duplicate, and noncanonical bootstrap fields are refused byte-identically.
cp "$APP/platform-v3.manifest" "$FIXTURE/manifest.good"
for case_name in malformed duplicate noncanonical; do
  cp "$FIXTURE/manifest.good" "$APP/platform-v3.manifest"
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
  if MISTER_MAGIK_TEST_KEYS=down run_manager >"$FIXTURE/bootstrap-$case_name.log" 2>&1; then
    echo "$case_name manifest unexpectedly ran" >&2
    exit 1
  fi
  assert_boot_unchanged
done
cp "$FIXTURE/manifest.good" "$APP/platform-v3.manifest"

# A missing manager and a corrupt manager are refused before boot files change.
printf 'exit 99\n' >"$FAT/Scripts/MiSTer-MagiK.platform-v3.constants.sh"
run_manager verify-platform >"$FIXTURE/stale-helper.log"
grep -q 'verified platform' "$FIXTURE/stale-helper.log"
assert_boot_unchanged

mv "$APP/mister-magik-manager" "$FIXTURE/manager.missing"
if MISTER_MAGIK_TEST_KEYS=down run_manager >"$FIXTURE/missing-manager.log" 2>&1; then
  echo "missing manager unexpectedly ran" >&2
  exit 1
fi
grep -q 'missing .*mister-magik-manager' "$FIXTURE/missing-manager.log"
assert_boot_unchanged
mv "$FIXTURE/manager.missing" "$APP/mister-magik-manager"

cp "$APP/mister-magik-manager" "$FIXTURE/manager.good"
printf 'corrupt\n' >>"$APP/mister-magik-manager"
if MISTER_MAGIK_TEST_KEYS=down run_manager >"$FIXTURE/corrupt.log" 2>&1; then
  echo "corrupt manager unexpectedly ran" >&2
  exit 1
fi
grep -q 'manager hash mismatch' "$FIXTURE/corrupt.log"
assert_boot_unchanged
cp "$FIXTURE/manager.good" "$APP/mister-magik-manager"
chmod 755 "$APP/mister-magik-manager"

# The direct diagnostic commands remain available alongside the user menu.
run_manager status >"$FIXTURE/status.log"
grep -q 'effective Main=Other' "$FIXTURE/status.log"
run_manager verify-platform >"$FIXTURE/verify-platform.log"
grep -q 'verified platform' "$FIXTURE/verify-platform.log"

# Install from stock, cancel the installed menu, restore stock, and reinstall.
new_fixture restore-reinstall
seed_package
MISTER_MAGIK_TEST_KEYS=down run_manager >"$FIXTURE/install.log"
grep -q 'installed. Rebooting to start MiSTer MagiK' "$FIXTURE/install.log"
grep -q 'TEST: normal reboot requested' "$FIXTURE/install.log"
cmp "$FIXTURE/original.ini" "$FAT/MiSTer.ini.bak.before-magik"
assert_magik_selected

BEFORE_INI="$(sha256sum "$FAT/MiSTer.ini")"
BEFORE_INITTAB="$(sha256sum "$INITTAB")"
MISTER_MAGIK_TEST_KEYS=cancel run_manager >"$FIXTURE/active-cancel.log"
assert_boot_unchanged
assert_magik_selected

printf 'after_restore=keep\r\n' >>"$FAT/MiSTer.ini"
MISTER_MAGIK_TEST_KEYS=enter,other run_manager >"$FIXTURE/restore.log"
grep -q 'stock MiSTer boot restored' "$FIXTURE/restore.log"
grep -q 'reboot skipped' "$FIXTURE/restore.log"
grep -q 'after_restore=keep' "$FAT/MiSTer.ini"
assert_stock
test -d "$APP"
test -f "$FAT/MiSTer.ini.bak.before-magik"

MISTER_MAGIK_TEST_KEYS=down run_manager >"$FIXTURE/reinstall.log"
grep -q 'installed. Rebooting to start MiSTer MagiK' "$FIXTURE/reinstall.log"
assert_magik_selected
grep -q 'after_restore=keep' "$FAT/MiSTer.ini"

# Restore's positive reboot choice is exercised independently from its skip path.
new_fixture restore-reboot
seed_package
MISTER_MAGIK_TEST_KEYS=down run_manager >/dev/null
MISTER_MAGIK_TEST_KEYS=enter,enter run_manager >"$FIXTURE/restore-reboot.log"
grep -q 'TEST: normal reboot requested' "$FIXTURE/restore-reboot.log"
assert_stock
test -d "$APP"

seed_unowned_files() {
  mkdir -p "$FAT/licenses" "$FAT/mister-magik-dev" "$FAT/Scripts/.mister-magik-user"
  printf 'stock\n' >"$FAT/MiSTer"
  printf 'stock menu\n' >"$FAT/menu.rbf"
  printf 'user backup\n' >"$FAT/MiSTer.ini.bak"
  printf 'developer\n' >"$FAT/MiSTer_MagiKDev"
  printf 'developer payload\n' >"$FAT/mister-magik-dev/keep.txt"
  printf 'user license\n' >"$FAT/licenses/USER-LICENSE.txt"
  printf 'user hook\n' >"$FAT/Scripts/.mister-magik-user/hook"
  local owned
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
}

# Full-uninstall confirmation cancellation leaves the active package untouched.
new_fixture uninstall-matrix
seed_package
MISTER_MAGIK_TEST_KEYS=down run_manager >/dev/null
seed_unowned_files
if MISTER_MAGIK_TEST_KEYS=down,enter,enter run_manager >"$FIXTURE/uninstall-cancel.log" 2>&1; then
  echo "Enter unexpectedly confirmed uninstall" >&2
  exit 1
fi
test -d "$APP"
assert_magik_selected

# Full uninstall restores stock, removes only owned files, and can skip reboot.
printf 'exit 99\n' >"$FAT/Scripts/MiSTer-MagiK.platform-v3.constants.sh"
MISTER_MAGIK_TEST_KEYS=down,enter,down,other run_manager >"$FIXTURE/uninstall.log"
grep -q 'fully uninstalled' "$FIXTURE/uninstall.log"
grep -q 'reboot skipped' "$FIXTURE/uninstall.log"
assert_owned_files_removed
assert_unowned_files_preserved
assert_stock

# A fresh Downloader copy can be seeded after full uninstall and installed again.
seed_package
MISTER_MAGIK_TEST_KEYS=down run_manager >"$FIXTURE/reseed-install.log"
grep -q 'installed. Rebooting to start MiSTer MagiK' "$FIXTURE/reseed-install.log"
assert_magik_selected
test -f "$FAT/MiSTer.ini.bak.before-magik"

# Full uninstall's positive reboot choice is also test-mode safe.
new_fixture uninstall-reboot
seed_package
MISTER_MAGIK_TEST_KEYS=down run_manager >/dev/null
MISTER_MAGIK_TEST_KEYS=down,enter,down,enter run_manager >"$FIXTURE/uninstall-reboot.log"
grep -q 'fully uninstalled' "$FIXTURE/uninstall-reboot.log"
grep -q 'TEST: normal reboot requested' "$FIXTURE/uninstall-reboot.log"
assert_owned_files_removed
assert_stock

# A registered MagiK database is removed through Downloader before the
# package-owned files are deleted. The fixture updater models the stable
# machine output contract and keeps unrelated Downloader state intact.
new_fixture downloader-registration
seed_package
MISTER_MAGIK_TEST_KEYS=down run_manager >/dev/null
mkdir -p "$FAT/Scripts/.config/downloader"
printf '{"unrelated":true}\n' >"$FAT/Scripts/.config/downloader/downloader.json"
printf 'registered\n' >"$FAT/Scripts/.config/downloader/registered"
printf '[MiSTer]\nallow_delete = 0\nallow_reboot = 0\nupdate_linux = false\n[mister_magik]\ndb_url = http://fixture/magik.json\n' >"$FAT/downloader.ini"
printf '[mister_magik]\ndb_url = http://fixture/magik.json\n' >"$FAT/downloader_mister_magik.ini"
printf 'managed payload\n' >"$FAT/mister-magik/managed.txt"
DOWNLOADER_FIXTURE="$FIXTURE/downloader-fixture.sh"
printf '%s\n' \
  '#!/bin/sh' \
  'set -eu' \
  'STATE="${MISTER_MAGIK_FAT}/Scripts/.config/downloader/registered"' \
  'case "${1:-}" in' \
  '  --version) printf "2.4.0\\n" ;;' \
  '  --list-dbs) if [ -f "$STATE" ]; then printf "DLP1\\tevent:installed_db\\tdb:mister_magik\\n"; fi ;;' \
  '  --uninstall) [ "${2:-}" = mister_magik ] || exit 2; rm -f "$STATE" ;;' \
  '  *) exit 2 ;;' \
  'esac' >"$DOWNLOADER_FIXTURE"
chmod 755 "$DOWNLOADER_FIXTURE"
MISTER_MAGIK_DOWNLOADER="$DOWNLOADER_FIXTURE" \
  MISTER_MAGIK_TEST_KEYS=down,enter,down,other run_manager >"$FIXTURE/downloader-registration.log"
if ! grep -q 'fully uninstalled' "$FIXTURE/downloader-registration.log"; then
  sed -n '1,160p' "$FIXTURE/downloader-registration.log" >&2
  exit 1
fi
test ! -e "$FAT/Scripts/.config/downloader/registered"
test -e "$FAT/Scripts/.config/downloader/downloader.json"
test ! -e "$FAT/mister-magik"

echo "installer lifecycle matrix: PASS"
