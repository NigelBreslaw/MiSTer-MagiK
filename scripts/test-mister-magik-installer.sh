#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT
FAT="$TMP/fat"
APP="$FAT/mister-magik"
mkdir -p "$APP/fpga" "$APP/art"

printf '#!/bin/sh\n' >"$FAT/MiSTer_MagiK"
printf '#!/bin/sh\n' >"$APP/mister-magik-fb"
printf '#!/bin/sh\n' >"$APP/mister-magik-catalog-builder"
printf 'module\n' >"$APP/mister_magik_scanout_slots.ko"
printf 'rbf\n' >"$APP/fpga/menu-magik-vblank-latch.rbf"
printf 'logo\n' >"$APP/art/slint-logo-pixel.rgba"
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
  --output "$APP/platform-v1.manifest" \
  --main "$FAT/MiSTer_MagiK" --gui "$APP/mister-magik-fb" \
  --catalog-builder "$APP/mister-magik-catalog-builder" \
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
    MISTER_MAGIK_TEST_MODE=1 MISTER_MAGIK_NO_PAUSE=1 \
    "$ROOT/scripts/mister-magik.sh" "$@"
}

run_installer install >/dev/null
grep -q '^main=MiSTer_MagiK$' "$FAT/MiSTer.ini"
cmp "$TMP/MiSTer.ini.before-install" "$FAT/MiSTer.ini.bak.before-magik"
test ! -e "$FAT/MiSTer.ini.bak"
grep -qx '::sysinit:/media/fat/MiSTer &' "$TMP/inittab"
test -x "$FAT/MiSTer_MagiK"
test -x "$APP/mister-magik-fb"

if command -v expect >/dev/null 2>&1; then
  export TEST_INSTALLER="$ROOT/scripts/mister-magik.sh"
  export TEST_FAT="$FAT"
  export TEST_INITTAB="$TMP/inittab"
  expect <<'EOF' >/dev/null
log_user 0
set timeout 5
spawn env MISTER_MAGIK_FAT=$env(TEST_FAT) MISTER_MAGIK_INITTAB=$env(TEST_INITTAB) MISTER_MAGIK_TEST_MODE=1 MISTER_MAGIK_NO_PAUSE=1 $env(TEST_INSTALLER)
expect "Press UP to reinstall or DOWN to disable MagiK boot."
send -- "\033\[A"
expect "Press A/Enter to confirm"
send -- "\r"
expect "installed. Reboot to start MiSTer MagiK."
expect eof
EOF
  cmp "$TMP/MiSTer.ini.before-install" "$FAT/MiSTer.ini.bak.before-magik"
fi

printf '\ncustom_after_install=1\n' >>"$FAT/MiSTer.ini"
run_installer install >/dev/null
cmp "$TMP/MiSTer.ini.before-install" "$FAT/MiSTer.ini.bak.before-magik"
grep -q '^custom_after_install=1$' "$FAT/MiSTer.ini"

rm "$FAT/MiSTer.ini.bak.before-magik"
run_installer install >"$TMP/reinstall-without-backup.log"
test ! -e "$FAT/MiSTer.ini.bak.before-magik"
grep -q 'not creating it from a MagiK-active MiSTer.ini' "$TMP/reinstall-without-backup.log"

run_installer disable >/dev/null
if grep -q '^main=MiSTer_MagiK$' "$FAT/MiSTer.ini"; then
  echo "disable left MagiK main enabled" >&2
  exit 1
fi
test "$(grep -c '^main=MiSTer$' "$FAT/MiSTer.ini")" = 1
test -f "$APP/mister-magik-fb"

ini_before="$(sha256sum "$FAT/MiSTer.ini")"
inittab_before="$(sha256sum "$TMP/inittab")"
printf 'corrupt\n' >>"$APP/mister-magik-fb"
if run_installer install >/dev/null 2>&1; then
  echo "corrupt platform unexpectedly installed" >&2
  exit 1
fi
test "$(sha256sum "$FAT/MiSTer.ini")" = "$ini_before"
test "$(sha256sum "$TMP/inittab")" = "$inittab_before"

echo "mister-magik installer self-test ok"
