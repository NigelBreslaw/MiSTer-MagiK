#!/usr/bin/env bash
# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

# Strict remote verification for the installed platform-v2 manifest. The
# manifest records installed paths; artifact_suffix selects inactive uploads.

platform_manifest_verify() {
  local mister="$1" layout="$2" manifest="$3" artifact_suffix="${4:-}"
  local mode="${5:-verify}" candidate_gui_sha="${6:-}" app main
  case "$layout" in
    dev) app=/media/fat/mister-magik-dev; main=/media/fat/MiSTer_MagiKDev ;;
    public) app=/media/fat/mister-magik; main=/media/fat/MiSTer_MagiK ;;
    *) echo "unknown platform layout: $layout" >&2; return 2 ;;
  esac
  "$mister" run "
set -e
manifest='$manifest'
suffix='$artifact_suffix'
mode='$mode'
candidate_gui_sha='$candidate_gui_sha'
tmp=\"\$manifest.runtime-upload\"
trap 'rm -f \"\$tmp\"' EXIT
get() { value=\$(sed -n \"s/^\$1=//p\" \"\$manifest\"); test -n \"\$value\"; test \"\$(grep -c \"^\$1=\" \"\$manifest\")\" -eq 1; printf '%s' \"\$value\"; }
expected_fields='format main_path gui_path scanout_module_path scanout_metadata_path latch_rbf_path latch_metadata_path main_sha256 gui_sha256 scanout_module_sha256 scanout_metadata_sha256 latch_rbf_sha256 latch_metadata_sha256 platform_contract_sha256 main_revision magik_revision menu_revision'
records=\$(awk 'NF && \$0 !~ /^#/ { count++ } END { print count + 0 }' \"\$manifest\")
test \"\$records\" -eq 17
for field in \$expected_fields; do get \"\$field\" >/dev/null; done
test \"\$(get format)\" = mister-magik-platform-v2
test \"\$(get main_path)\" = '$main'
test \"\$(get gui_path)\" = '$app/mister-magik-fb'
test \"\$(get scanout_module_path)\" = '$app/mister_magik_scanout_slots.ko'
test \"\$(get scanout_metadata_path)\" = '$app/mister_magik_scanout_slots.metadata.txt'
test \"\$(get latch_rbf_path)\" = '$app/fpga/menu-magik-vblank-latch.rbf'
test \"\$(get latch_metadata_path)\" = '$app/fpga/menu-magik-vblank-latch.metadata.txt'
is_hex() { value=\$1; width=\$2; test \"\${#value}\" -eq \"\$width\"; echo \"\$value\" | grep -Eq '^[0-9a-f]+\$'; }
for field in main_sha256 gui_sha256 scanout_module_sha256 scanout_metadata_sha256 latch_rbf_sha256 latch_metadata_sha256 platform_contract_sha256; do is_hex \"\$(get \"\$field\")\" 64; done
for field in main_revision magik_revision menu_revision; do is_hex \"\$(get \"\$field\")\" 40; done
check() { path=\$1; key=\$2; test -r \"\$path\$suffix\"; test \"\$(sha256sum \"\$path\$suffix\" | awk '{print \$1}')\" = \"\$(get \"\$key\")\"; }
check '$main' main_sha256
check '$app/mister_magik_scanout_slots.ko' scanout_module_sha256
check '$app/mister_magik_scanout_slots.metadata.txt' scanout_metadata_sha256
check '$app/fpga/menu-magik-vblank-latch.rbf' latch_rbf_sha256
check '$app/fpga/menu-magik-vblank-latch.metadata.txt' latch_metadata_sha256
contract=\$(get platform_contract_sha256)
module_hash=\$(get scanout_module_sha256)
rbf_hash=\$(get latch_rbf_sha256)
menu_revision=\$(get menu_revision)
grep -qx \"platform_contract_sha256=\$contract\" '$app/mister_magik_scanout_slots.metadata.txt'\"\$suffix\"
grep -qx \"platform_contract_sha256=\$contract\" '$app/fpga/menu-magik-vblank-latch.metadata.txt'\"\$suffix\"
grep -qx \"module_sha256=\$module_hash\" '$app/mister_magik_scanout_slots.metadata.txt'\"\$suffix\"
grep -qx \"rbf_sha256=\$rbf_hash\" '$app/fpga/menu-magik-vblank-latch.metadata.txt'\"\$suffix\"
grep -qx \"source_commit=\$menu_revision\" '$app/fpga/menu-magik-vblank-latch.metadata.txt'\"\$suffix\"
case \"\$mode\" in
  verify) check '$app/mister-magik-fb' gui_sha256 ;;
  rebind)
    test -z \"\$suffix\"
    is_hex \"\$candidate_gui_sha\" 64
    test \"\$(sha256sum '$app/mister-magik-fb' | awk '{print \$1}')\" = \"\$candidate_gui_sha\"
    awk -v hash=\"\$candidate_gui_sha\" 'BEGIN { seen = 0 } /^gui_sha256=/ { print \"gui_sha256=\" hash; seen++; next } { print } END { if (seen != 1) exit 1 }' \"\$manifest\" >\"\$tmp\"
    test \"\$(awk 'NF && \$0 !~ /^#/ { count++ } END { print count + 0 }' \"\$tmp\")\" -eq 17
    sync
    mv \"\$tmp\" \"\$manifest\"
    sync
    ;;
  *) exit 2 ;;
esac
"
}

platform_manifest_self_test() {
  local tmp fake log
  tmp="$(mktemp -d "${TMPDIR:-/tmp}/platform-manifest-lib.XXXXXX")"
  fake="$tmp/mister"
  log="$tmp/log"
  printf '#!/usr/bin/env bash\nprintf "%%s" "$2" >"$PLATFORM_MANIFEST_TEST_LOG"\n' >"$fake"
  chmod +x "$fake"
  PLATFORM_MANIFEST_TEST_LOG="$log" platform_manifest_verify "$fake" dev /media/fat/mister-magik-dev/platform-v2.manifest .upload verify
  grep -q 'records.*-eq 17' "$log"
  grep -q 'source_commit=.*menu_revision' "$log"
  grep -q 'mister_magik_scanout_slots.metadata.txt' "$log"
  grep -q 'suffix=.upload' "$log"
  rm -rf "$tmp"
  echo "platform manifest library self-test ok"
}

if [[ "${BASH_SOURCE[0]}" == "$0" ]]; then
  case "${1:-}" in
    --self-test) platform_manifest_self_test ;;
    *) echo "usage: $0 --self-test" >&2; exit 2 ;;
  esac
fi
