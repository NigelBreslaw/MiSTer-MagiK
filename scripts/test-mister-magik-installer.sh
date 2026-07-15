#!/usr/bin/env bash
# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT
FAT="$TMP/fat"
APP="$FAT/mister-magik"
INIT_DIR="$TMP/init.d"
mkdir -p "$APP/fpga" "$FAT/Scripts" "$INIT_DIR"

printf '#!/bin/sh\n' >"$FAT/MiSTer_MagiK"
printf '#!/bin/sh\n' >"$APP/mister-magik-fb"
printf 'module\n' >"$APP/mister_magik_scanout_slots.ko"
printf 'rbf\n' >"$APP/fpga/menu-magik-vblank-latch.rbf"
contract="$(printf contract | sha256sum | awk '{print $1}')"
magik="1111111111111111111111111111111111111111"
main="2222222222222222222222222222222222222222"
menu="3333333333333333333333333333333333333333"
module_hash="$(sha256sum "$APP/mister_magik_scanout_slots.ko" | awk '{print $1}')"
rbf_hash="$(sha256sum "$APP/fpga/menu-magik-vblank-latch.rbf" | awk '{print $1}')"
printf 'platform_contract_sha256=%s\nmodule_sha256=%s\nvermagic=5.15.1-MiSTer fixture\n' \
  "$contract" "$module_hash" >"$APP/mister_magik_scanout_slots.metadata.txt"
printf 'format=mister-magik-fpga-release-v1\nplatform_contract_sha256=%s\nmagik_commit=%s\nsource_commit=%s\nrbf_sha256=%s\n' \
  "$contract" "$magik" "$menu" "$rbf_hash" >"$APP/fpga/menu-magik-vblank-latch.metadata.txt"
"$ROOT/scripts/platform-manifest.py" generate \
  --output "$APP/platform-v2.manifest" \
  --main "$FAT/MiSTer_MagiK" --gui "$APP/mister-magik-fb" \
  --scanout-module "$APP/mister_magik_scanout_slots.ko" \
  --scanout-metadata "$APP/mister_magik_scanout_slots.metadata.txt" \
  --latch-rbf "$APP/fpga/menu-magik-vblank-latch.rbf" \
  --latch-metadata "$APP/fpga/menu-magik-vblank-latch.metadata.txt" \
  --main-revision "$main" --magik-revision "$magik" >/dev/null

printf '[MiSTer]\ndirect_video=0\n' >"$FAT/MiSTer.ini"
cp "$FAT/MiSTer.ini" "$TMP/MiSTer.ini.before-install"
printf '::sysinit:/media/fat/MiSTer &\n' >"$TMP/inittab"
run_installer() {
  MISTER_MAGIK_FAT="$FAT" MISTER_MAGIK_INITTAB="$TMP/inittab" \
    MISTER_MAGIK_INIT_DIR="$INIT_DIR" \
    MISTER_MAGIK_TEST_MODE=1 MISTER_MAGIK_TEST_CONFIRM_INSTALL=1 \
    MISTER_MAGIK_NO_PAUSE=1 \
    "$ROOT/scripts/MiSTer-MagiK.sh" "$@"
}

run_confirmed_uninstall() {
  MISTER_MAGIK_FAT="$FAT" MISTER_MAGIK_INITTAB="$TMP/inittab" \
    MISTER_MAGIK_INIT_DIR="$INIT_DIR" \
    MISTER_MAGIK_TEST_MODE=1 MISTER_MAGIK_TEST_CONFIRM_UNINSTALL=1 \
    MISTER_MAGIK_NO_PAUSE=1 "$ROOT/scripts/MiSTer-MagiK.sh" uninstall
}

run_installer_with_keys() {
  keys="$1"
  shift
  MISTER_MAGIK_FAT="$FAT" MISTER_MAGIK_INITTAB="$TMP/inittab" \
    MISTER_MAGIK_INIT_DIR="$INIT_DIR" MISTER_MAGIK_TEST_MODE=1 \
    MISTER_MAGIK_TEST_CONFIRM_INSTALL=1 MISTER_MAGIK_TEST_KEYS="$keys" \
    MISTER_MAGIK_TEST_REBOOT_TRACE="${MISTER_MAGIK_TEST_REBOOT_TRACE:-}" \
    MISTER_MAGIK_NO_PAUSE=1 "$ROOT/scripts/MiSTer-MagiK.sh" "$@"
}

assert_one_main() {
  expected="$1"
  awk -v expected="$expected" '
    BEGIN { section = ""; count = 0; selected = "" }
    function is_main_assignment(line, key) {
      if (line !~ /=/) return 0
      key = line
      sub(/=.*/, "", key)
      gsub(/[[:space:]]/, "", key)
      return tolower(key) == "main"
    }
    { sub(/\r$/, "") }
    /^[[:space:]]*\[[^]]+\]/ {
      section = $0
      sub(/^[[:space:]]*\[/, "", section)
      sub(/\].*$/, "", section)
      gsub(/[[:space:]]/, "", section)
      next
    }
    tolower(section) == "mister" && is_main_assignment($0) {
      value = $0
      sub(/^[^=]*=[[:space:]]*/, "", value)
      sub(/[[:space:]]*[;#].*$/, "", value)
      gsub(/[[:space:]]/, "", value)
      selected = value
      count++
    }
    END { exit(count == 1 && selected == expected ? 0 : 1) }
  ' "$FAT/MiSTer.ini"
}

assert_menu_1080p() {
  awk '
    BEGIN { section = ""; count = 0; selected = "" }
    function is_video_mode_assignment(line, key) {
      if (line !~ /=/) return 0
      key = line
      sub(/=.*/, "", key)
      gsub(/[[:space:]]/, "", key)
      return tolower(key) == "video_mode"
    }
    { sub(/\r$/, "") }
    /^[[:space:]]*\[[^]]+\]/ {
      section = $0
      sub(/^[[:space:]]*\[/, "", section)
      sub(/\].*$/, "", section)
      gsub(/[[:space:]]/, "", section)
      next
    }
    tolower(section) == "menu" && is_video_mode_assignment($0) {
      value = $0
      sub(/^[^=]*=[[:space:]]*/, "", value)
      sub(/[[:space:]]*[;#].*$/, "", value)
      gsub(/[[:space:]]/, "", value)
      selected = value
      count++
    }
    END { exit(count == 1 && selected == "8" ? 0 : 1) }
  ' "$FAT/MiSTer.ini"
}

# Installation must be confirmed before verification or configuration changes.
ini_before_confirmation="$(sha256sum "$FAT/MiSTer.ini")"
inittab_before_confirmation="$(sha256sum "$TMP/inittab")"
if MISTER_MAGIK_FAT="$FAT" MISTER_MAGIK_INITTAB="$TMP/inittab" \
  MISTER_MAGIK_INIT_DIR="$INIT_DIR" MISTER_MAGIK_TEST_MODE=1 \
  MISTER_MAGIK_NO_PAUSE=1 "$ROOT/scripts/MiSTer-MagiK.sh" install \
  >"$TMP/noninteractive-install.log" 2>&1; then
  echo "noninteractive install unexpectedly succeeded" >&2
  exit 1
fi
grep -q 'currently only supports 1080p' "$TMP/noninteractive-install.log"
grep -q 'interactive input is unavailable; installation refused' "$TMP/noninteractive-install.log"
test "$(sha256sum "$FAT/MiSTer.ini")" = "$ini_before_confirmation"
test "$(sha256sum "$TMP/inittab")" = "$inittab_before_confirmation"

run_installer install >/dev/null
assert_one_main MiSTer_MagiK
assert_menu_1080p
cmp "$TMP/MiSTer.ini.before-install" "$FAT/MiSTer.ini.bak.before-magik"
test ! -e "$FAT/MiSTer.ini.bak"
grep -qx '::sysinit:/media/fat/MiSTer &' "$TMP/inittab"
test -x "$FAT/MiSTer_MagiK"
test -x "$APP/mister-magik-fb"

# The installed-state menu is testable without optional host PTY tooling.
ini_before_cancel="$(sha256sum "$FAT/MiSTer.ini")"
run_installer_with_keys cancel >/dev/null
test "$(sha256sum "$FAT/MiSTer.ini")" = "$ini_before_cancel"
MISTER_MAGIK_TEST_REBOOT_TRACE="$TMP/reboot.trace" \
  run_installer_with_keys enter,enter >"$TMP/restore-reboot.log"
assert_one_main MiSTer
assert_menu_1080p
test -f "$FAT/MiSTer.ini.bak.before-magik"
grep -q 'TEST: normal reboot requested' "$TMP/restore-reboot.log"
test "$(cat "$TMP/reboot.trace")" = "$(printf 'sync\nreboot')"
rm -f "$TMP/reboot.trace"
MISTER_MAGIK_TEST_REBOOT_TRACE="$TMP/reboot.trace" \
  run_installer_with_keys other install >"$TMP/install-reboot-skip.log"
grep -q 'reboot skipped' "$TMP/install-reboot-skip.log"
test ! -e "$TMP/reboot.trace"
if run_installer_with_keys down,enter,other >"$TMP/menu-uninstall-cancel.log" 2>&1; then
  echo "menu uninstall cancellation unexpectedly succeeded" >&2
  exit 1
fi
grep -q 'cancelled; no changes made' "$TMP/menu-uninstall-cancel.log"
assert_one_main MiSTer_MagiK
test -d "$APP"

if command -v expect >/dev/null 2>&1; then
  export TEST_INSTALLER="$ROOT/scripts/MiSTer-MagiK.sh"
  export TEST_FAT="$FAT"
  export TEST_INITTAB="$TMP/inittab"
  expect <<'EOF' >/dev/null
log_user 0
set timeout 5
spawn env MISTER_MAGIK_FAT=$env(TEST_FAT) MISTER_MAGIK_INITTAB=$env(TEST_INITTAB) MISTER_MAGIK_TEST_MODE=1 MISTER_MAGIK_NO_PAUSE=1 $env(TEST_INSTALLER)
expect "> Restore stock MiSTer"
send -- "\r"
expect "stock MiSTer boot restored"
expect eof
EOF
  cmp "$TMP/MiSTer.ini.before-install" "$FAT/MiSTer.ini.bak.before-magik"
  assert_one_main MiSTer
  assert_menu_1080p
  expect <<'EOF' >/dev/null
log_user 0
set timeout 5
spawn env MISTER_MAGIK_FAT=$env(TEST_FAT) MISTER_MAGIK_INITTAB=$env(TEST_INITTAB) MISTER_MAGIK_TEST_MODE=1 MISTER_MAGIK_NO_PAUSE=1 $env(TEST_INSTALLER) install
expect "currently only supports 1080p"
expect "Continue by pressing A/Enter"
send -- "x"
expect "cancelled; no changes made"
expect eof
EOF
  assert_one_main MiSTer
  assert_menu_1080p
  run_installer install >/dev/null

  expect <<'EOF' >/dev/null
log_user 0
set timeout 5
spawn env MISTER_MAGIK_FAT=$env(TEST_FAT) MISTER_MAGIK_INITTAB=$env(TEST_INITTAB) MISTER_MAGIK_TEST_MODE=1 MISTER_MAGIK_NO_PAUSE=1 $env(TEST_INSTALLER)
expect "> Restore stock MiSTer"
send -- "\033\[B"
expect "> Fully uninstall MiSTer MagiK"
send -- "\r"
expect "Press A/Enter to confirm"
send -- "x"
expect "cancelled; no changes made"
expect eof
EOF
  assert_one_main MiSTer_MagiK
  test -d "$APP"
fi

printf '\ncustom_after_install=1\n' >>"$FAT/MiSTer.ini"
run_installer install >/dev/null
cmp "$TMP/MiSTer.ini.before-install" "$FAT/MiSTer.ini.bak.before-magik"
grep -q '^custom_after_install=1$' "$FAT/MiSTer.ini"

rm "$FAT/MiSTer.ini.bak.before-magik"
run_installer install >"$TMP/reinstall-without-backup.log"
test ! -e "$FAT/MiSTer.ini.bak.before-magik"
grep -q 'not creating it from a MagiK-active MiSTer.ini' "$TMP/reinstall-without-backup.log"

# Main_MiSTer applies active keys in file order. A later non-MagiK value must
# trigger direct installation, and installation must collapse all duplicates.
mkdir -p "$FAT/licenses"
touch "$FAT/licenses/MiSTer-MagiK-GPL-3.0-or-later.txt"
touch "$FAT/licenses/RUST-LIBRARIES.txt"
touch "$FAT/licenses/FFMPEG-LGPL-2.1-or-later.txt"
touch "$FAT/licenses/PRESS-START-2P-OFL-1.1.txt"
touch "$FAT/THIRD-PARTY-NOTICES.txt" "$FAT/SOURCE-OFFER.txt"
printf 'keep\n' >"$FAT/licenses/USER-LICENSE.txt"
printf '[MiSTer]\r\nMAIN=MiSTer_MagiK\r\n;main=Commented\r\nmain=Other_Main\r\n[Menu] ; primary\r\nvideo_mode=5\r\n  custom_setting = keep ; untouched\r\n[MiSTer]\r\nmain=Another_Main ; note\r\n[Menu]\r\nVIDEO_MODE=6 ; old mode\r\n' >"$FAT/MiSTer.ini"
run_installer >"$TMP/effective-other.log"
grep -q 'installed. Reboot to start MiSTer MagiK.' "$TMP/effective-other.log"
assert_one_main MiSTer_MagiK
assert_menu_1080p
python3 - "$FAT/MiSTer.ini" <<'PY'
import pathlib, sys
data = pathlib.Path(sys.argv[1]).read_bytes()
assert b"\r\n" in data
assert b"\n" not in data.replace(b"\r\n", b"")
assert b"[Menu] ; primary\r\n" in data
assert b"  custom_setting = keep ; untouched\r\n" in data
PY
grep -q ';main=Commented' "$FAT/MiSTer.ini"
grep -q '^;main=Other_Main' "$FAT/MiSTer.ini"
grep -q '^;main=Another_Main ; note' "$FAT/MiSTer.ini"
grep -q '^video_mode=8' "$FAT/MiSTer.ini"
grep -q '^;VIDEO_MODE=6 ; old mode' "$FAT/MiSTer.ini"
test ! -e "$FAT/THIRD-PARTY-NOTICES.txt" && test ! -e "$FAT/SOURCE-OFFER.txt"
test ! -e "$FAT/licenses/MiSTer-MagiK-GPL-3.0-or-later.txt"
test -e "$FAT/licenses/USER-LICENSE.txt"

# Conversely, an earlier non-MagiK value followed by MagiK is installed state.
# Without a TTY the management menu must make no change.
printf '[MiSTer]\nmain=Other_Main\n[MiSTer]\nmain=MiSTer_MagiK\n' >"$FAT/MiSTer.ini"
ini_before_menu="$(sha256sum "$FAT/MiSTer.ini")"
run_installer >"$TMP/noninteractive-menu.log"
grep -q 'interactive input is unavailable; no changes made' "$TMP/noninteractive-menu.log"
test "$(sha256sum "$FAT/MiSTer.ini")" = "$ini_before_menu"

run_installer install >/dev/null
assert_one_main MiSTer_MagiK
run_installer restore >/dev/null
assert_one_main MiSTer
assert_menu_1080p
test -f "$APP/mister-magik-fb"
test -f "$FAT/MiSTer.ini.bak.before-magik"
run_installer install >/dev/null

ini_before="$(sha256sum "$FAT/MiSTer.ini")"
inittab_before="$(sha256sum "$TMP/inittab")"
printf 'corrupt\n' >>"$APP/mister-magik-fb"
if run_installer install >/dev/null 2>&1; then
  echo "corrupt platform unexpectedly installed" >&2
  exit 1
fi
test "$(sha256sum "$FAT/MiSTer.ini")" = "$ini_before"
test "$(sha256sum "$TMP/inittab")" = "$inittab_before"

# Explicit uninstall requires a TTY and must not restore or delete anything
# before confirmation has succeeded.
if run_installer uninstall >"$TMP/noninteractive-uninstall.log" 2>&1; then
  echo "noninteractive uninstall unexpectedly succeeded" >&2
  exit 1
fi
grep -q 'interactive input is unavailable; uninstall refused' "$TMP/noninteractive-uninstall.log"
test -d "$APP"
assert_one_main MiSTer_MagiK

# A boot-restore failure must likewise leave every installed payload in place.
if MISTER_MAGIK_FAT="$FAT" MISTER_MAGIK_INITTAB="$TMP/missing/inittab" \
  MISTER_MAGIK_INIT_DIR="$INIT_DIR" MISTER_MAGIK_TEST_MODE=1 \
  MISTER_MAGIK_TEST_CONFIRM_UNINSTALL=1 MISTER_MAGIK_NO_PAUSE=1 \
  "$ROOT/scripts/MiSTer-MagiK.sh" uninstall >"$TMP/failed-restore.log" 2>&1; then
  echo "uninstall unexpectedly survived boot-restore failure" >&2
  exit 1
fi
test -d "$APP"
test -f "$FAT/MiSTer_MagiK"

# Restore the valid platform payload after the corruption test, then populate
# every owned legacy/current path plus unrelated files that must survive.
printf '#!/bin/sh\n' >"$APP/mister-magik-fb"
chmod +x "$APP/mister-magik-fb"
cp "$ROOT/scripts/MiSTer-MagiK.sh" "$FAT/Scripts/MiSTer-MagiK.sh"
touch "$FAT/Scripts/mister-magik.sh" "$FAT/Scripts/mister-magik-channel.sh"
printf '[mister_magik]\n' >"$FAT/downloader_mister_magik.ini"
touch "$FAT/downloader_mister_magik.ini.tmp.12" "$FAT/.downloader_mister_magik.ini.tmp.13"
touch "$FAT/.MiSTer.ini.bak.before-magik.new.12" "$FAT/.MiSTer.ini.magik.new.stale"
mkdir -p "$FAT/licenses"
touch "$FAT/licenses/MiSTer-MagiK-GPL-3.0-or-later.txt"
touch "$FAT/licenses/RUST-LIBRARIES.txt"
touch "$FAT/licenses/FFMPEG-LGPL-2.1-or-later.txt"
touch "$FAT/licenses/PRESS-START-2P-OFL-1.1.txt"
printf 'keep\n' >"$FAT/licenses/USER-LICENSE.txt"
touch "$FAT/THIRD-PARTY-NOTICES.txt" "$FAT/SOURCE-OFFER.txt"
printf '#!/bin/sh\n' >"$INIT_DIR/S00magik-agent"
printf '#!/bin/sh\n' >"$INIT_DIR/disabled-S00fastnet.magik-agent"
mkdir -p "$FAT/mister-magik-dev"
printf 'dev binary\n' >"$FAT/MiSTer_MagiKDev"
printf 'dev app\n' >"$FAT/mister-magik-dev/mister-magik-fb"
printf 'stock main\n' >"$FAT/MiSTer"
printf 'stock menu\n' >"$FAT/menu.rbf"
printf 'developer backup\n' >"$FAT/MiSTer.ini.bak"

run_confirmed_uninstall >"$TMP/uninstall.log"
assert_one_main MiSTer
assert_menu_1080p
grep -qx '::sysinit:/media/fat/MiSTer &' "$TMP/inittab"
test ! -e "$APP"
test ! -e "$FAT/MiSTer_MagiK"
test ! -e "$FAT/Scripts/MiSTer-MagiK.sh"
test ! -e "$FAT/Scripts/mister-magik.sh"
test ! -e "$FAT/Scripts/mister-magik-channel.sh"
test ! -e "$FAT/downloader_mister_magik.ini"
test ! -e "$FAT/MiSTer.ini.bak.before-magik"
test -e "$INIT_DIR/S00magik-agent"
test -e "$INIT_DIR/disabled-S00fastnet.magik-agent"
test -e "$FAT/MiSTer_MagiKDev"
test -e "$FAT/mister-magik-dev/mister-magik-fb"
test ! -e "$INIT_DIR/S00fastnet"
test -e "$FAT/MiSTer" && test -e "$FAT/menu.rbf"
test -e "$FAT/MiSTer.ini.bak"
test -e "$FAT/licenses/USER-LICENSE.txt"
test ! -e "$FAT/THIRD-PARTY-NOTICES.txt" && test ! -e "$FAT/SOURCE-OFFER.txt"
grep -q 'Reboot now? Press A/Enter to reboot' "$TMP/uninstall.log"
grep -q 'interactive input is unavailable; reboot not requested' "$TMP/uninstall.log"

# Full uninstall is idempotent and must not recreate persistent MagiK state.
run_confirmed_uninstall >/dev/null
assert_one_main MiSTer
assert_menu_1080p
test ! -e "$APP"
test -e "$FAT/licenses/USER-LICENSE.txt"

# A path that cannot be removed with the expected file operation must produce
# a nonzero result and name the residue, while leaving stock boot selected.
mkdir -p "$FAT/downloader_mister_magik.ini"
touch "$FAT/downloader_mister_magik.ini/unexpected-child"
if run_confirmed_uninstall >"$TMP/residue.log" 2>&1; then
  echo "uninstall unexpectedly ignored residue" >&2
  exit 1
fi
grep -q "uninstall residue: $FAT/downloader_mister_magik.ini" "$TMP/residue.log"
grep -q 'uninstall incomplete' "$TMP/residue.log"
assert_one_main MiSTer
assert_menu_1080p
rm -rf "$FAT/downloader_mister_magik.ini"

echo "mister-magik installer self-test ok"
