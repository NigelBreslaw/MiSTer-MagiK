#!/usr/bin/env bash
# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

# Cross-build the Rust frontend and deploy the binary to the MiSTer.
#
# This is a development-layout file deploy: when Main_MiSTer supervises the
# launcher, deploy asks it to suspend MagiK, swaps the binary, then resumes.
#
#   MISTER_IP=192.168.1.117 MISTER_PASS=1 scripts/deploy-rust.sh
#   MISTER_IP=... scripts/deploy-rust.sh --all-scenes     # lab/bench build
#   MISTER_IP=... scripts/deploy-rust.sh --experiments    # lab/bench build
#   MISTER_IP=... scripts/deploy-rust.sh --bench-tools    # benchmark command build
#   MISTER_IP=... scripts/deploy-rust.sh --diagnostics    # diagnostics command build
#   MISTER_IP=... scripts/deploy-rust.sh --ui-scope launcher
#   MISTER_DEPLOY_TRANSPORT=ssh scripts/deploy-rust.sh  # explicit fallback only
#
# Default installs the release-device (A3) binary.
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
source "$HERE/scripts/lib/bench-context-lib.sh"
source "$HERE/scripts/lib/magik-layout.sh"
magik_layout_select dev
REMOTE_DIR="$MISTER_MAGIK_APP_DIR"
REMOTE="$REMOTE_DIR/mister-magik-fb"
REMOTE_SCANOUT_MODULE="$REMOTE_DIR/mister_magik_scanout_slots.ko"
REMOTE_SCANOUT_METADATA="$REMOTE_DIR/mister_magik_scanout_slots.metadata.txt"
REMOTE_LATCH_RBF="$REMOTE_DIR/fpga/menu-magik-vblank-latch.rbf"
REMOTE_LATCH_METADATA="$REMOTE_DIR/fpga/menu-magik-vblank-latch.metadata.txt"
DEPLOY_TRANSPORT="${MISTER_DEPLOY_TRANSPORT:-agent}"

PROFILE=release-device
BUILD_FLAG=(--device)
UI_SCOPE="${MISTER_UI_BUILD_SCOPE:-all}"
ARGS=("$@")
for ((i = 0; i < ${#ARGS[@]}; i++)); do
  arg="${ARGS[$i]}"
  case "$arg" in
    --device) PROFILE=release-device; BUILD_FLAG=(--device) ;;
    --video|--mame-metadata|--hbmame-metadata|--asset-packs)
      echo "ERROR: $arg was removed from scripts/deploy-rust.sh; deploy runtime here and use catalog/media build tools explicitly" >&2
      exit 2
      ;;
    --all-scenes) UI_SCOPE=all; BUILD_FLAG+=(--all-scenes) ;;
    --experiments) UI_SCOPE=all; BUILD_FLAG+=(--experiments) ;;
    --bench-tools) BUILD_FLAG+=(--bench-tools) ;;
    --diagnostics) BUILD_FLAG+=(--diagnostics) ;;
    --ui-scope=*) UI_SCOPE="${arg#--ui-scope=}"; BUILD_FLAG+=("$arg") ;;
    --ui-scope)
      i=$((i + 1))
      if [ "$i" -ge "${#ARGS[@]}" ]; then
        echo "ERROR: --ui-scope requires one of: launcher, arcade, all" >&2
        exit 2
      fi
      UI_SCOPE="${ARGS[$i]}"
      BUILD_FLAG+=(--ui-scope "${ARGS[$i]}")
      ;;
    -h|--help)
      sed -n '2,13p' "$0" | sed 's/^# \{0,1\}//'
      exit 0
      ;;
    *)
      echo "ERROR: unknown argument: $arg" >&2
      exit 2
      ;;
  esac
done

BIN="$HERE/magik-gui/target/armv7-unknown-linux-gnueabihf/$PROFILE/mister-magik-fb"

bytes() {
  stat -f%z "$1" 2>/dev/null || stat -c%s "$1"
}

human_bytes() {
  awk -v b="$1" 'BEGIN {
    split("B KiB MiB GiB", u, " ");
    n = b + 0;
    i = 1;
    while (n >= 1024 && i < 4) { n /= 1024; i++ }
    if (i == 1) printf "%d %s", n, u[i];
    else printf "%.2f %s", n, u[i];
  }'
}

verify_dev_platform_manifest() {
  local mode="$1"
  local candidate_gui_sha="${2:-}"
  "$HERE/scripts/mister" run "
set -e
manifest='$MISTER_MAGIK_MANIFEST'
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
test \"\$(get main_path)\" = '$MISTER_MAGIK_MAIN'
test \"\$(get gui_path)\" = '$REMOTE'
test \"\$(get scanout_module_path)\" = '$REMOTE_SCANOUT_MODULE'
test \"\$(get scanout_metadata_path)\" = '$REMOTE_SCANOUT_METADATA'
test \"\$(get latch_rbf_path)\" = '$REMOTE_LATCH_RBF'
test \"\$(get latch_metadata_path)\" = '$REMOTE_LATCH_METADATA'
is_hex() { value=\$1; width=\$2; test \"\${#value}\" -eq \"\$width\"; echo \"\$value\" | grep -Eq '^[0-9a-f]+\$'; }
for field in main_sha256 gui_sha256 scanout_module_sha256 scanout_metadata_sha256 latch_rbf_sha256 latch_metadata_sha256 platform_contract_sha256; do is_hex \"\$(get \"\$field\")\" 64; done
for field in main_revision magik_revision menu_revision; do is_hex \"\$(get \"\$field\")\" 40; done
check() { path=\$1; key=\$2; test -r \"\$path\"; test \"\$(sha256sum \"\$path\" | awk '{print \$1}')\" = \"\$(get \"\$key\")\"; }
check '$MISTER_MAGIK_MAIN' main_sha256
check '$REMOTE_SCANOUT_MODULE' scanout_module_sha256
check '$REMOTE_SCANOUT_METADATA' scanout_metadata_sha256
check '$REMOTE_LATCH_RBF' latch_rbf_sha256
check '$REMOTE_LATCH_METADATA' latch_metadata_sha256
contract=\$(get platform_contract_sha256)
module_hash=\$(get scanout_module_sha256)
rbf_hash=\$(get latch_rbf_sha256)
menu_revision=\$(get menu_revision)
grep -qx \"platform_contract_sha256=\$contract\" '$REMOTE_SCANOUT_METADATA'
grep -qx \"platform_contract_sha256=\$contract\" '$REMOTE_LATCH_METADATA'
grep -qx \"module_sha256=\$module_hash\" '$REMOTE_SCANOUT_METADATA'
grep -qx \"rbf_sha256=\$rbf_hash\" '$REMOTE_LATCH_METADATA'
grep -qx \"source_commit=\$menu_revision\" '$REMOTE_LATCH_METADATA'
case \"\$mode\" in
  verify)
    check '$REMOTE' gui_sha256
    ;;
  rebind)
    is_hex \"\$candidate_gui_sha\" 64
    test \"\$(sha256sum '$REMOTE' | awk '{print \$1}')\" = \"\$candidate_gui_sha\"
    awk -v hash=\"\$candidate_gui_sha\" '
      BEGIN { seen = 0 }
      /^gui_sha256=/ { print \"gui_sha256=\" hash; seen++; next }
      { print }
      END { if (seen != 1) exit 1 }
    ' \"\$manifest\" > \"\$tmp\"
    test \"\$(awk 'NF && \$0 !~ /^#/ { count++ } END { print count + 0 }' \"\$tmp\")\" -eq 17
    test \"\$(grep -c \"^gui_sha256=\$candidate_gui_sha\$\" \"\$tmp\")\" -eq 1
    sync
    mv \"\$tmp\" \"\$manifest\"
    sync
    ;;
  *) exit 2 ;;
esac
"
}

echo "==> Cross-building (armv7 profile=$PROFILE)"
"$HERE/magik-gui/build-arm.sh" "${BUILD_FLAG[@]}"

LOCAL_BYTES="$(bytes "$BIN")"
LOCAL_SHA256="$(bench_context_sha256_file "$BIN")"
echo "==> Local binary size: $LOCAL_BYTES bytes ($(human_bytes "$LOCAL_BYTES"))"

echo "==> Preflighting development platform manifest"
verify_dev_platform_manifest verify

echo "==> Deploying $BIN -> $REMOTE via $DEPLOY_TRANSPORT"
DEPLOY_OUTPUT=""
case "$DEPLOY_TRANSPORT" in
  agent)
    DEPLOY_OUTPUT="$(
      MISTER_IP="${MISTER_IP:-192.168.1.117}" \
      MISTER_PASS="${MISTER_PASS:-1}" \
        "$HERE/scripts/mister" agent deploy-magik-bin "$BIN" "$REMOTE"
    )"
    ;;
  ssh)
    echo "==> Using explicit SSH/SFTP deploy fallback" >&2
    DEPLOY_OUTPUT="$(
      MISTER_IP="${MISTER_IP:-192.168.1.117}" \
      MISTER_PASS="${MISTER_PASS:-1}" \
        "$HERE/scripts/mister" deploy-magik-bin "$BIN" "$REMOTE"
    )"
    ;;
  *)
    echo "ERROR: unsupported MISTER_DEPLOY_TRANSPORT=$DEPLOY_TRANSPORT (expected agent or ssh)" >&2
    exit 2
    ;;
esac
printf '%s\n' "$DEPLOY_OUTPUT"
REMOTE_BYTES="$(
  printf '%s\n' "$DEPLOY_OUTPUT" \
    | awk '{ for (i = 1; i <= NF; i++) if ($i ~ /^remote_bytes=/) { sub(/^remote_bytes=/, "", $i); print $i } }' \
    | tail -1
)"
if [ -n "$REMOTE_BYTES" ]; then
  echo "==> Deployed binary size: $REMOTE_BYTES bytes ($(human_bytes "$REMOTE_BYTES"))"
fi
REMOTE_SHA256="$(bench_context_remote_sha256 "$HERE/scripts/mister" "$REMOTE" || true)"
REMOTE_SHA256="${REMOTE_SHA256:-missing}"
BUILT_FEATURES="$(bench_context_binary_features "$BIN")"
if ! bench_context_require_binary_contract "$BIN" "$REMOTE_SHA256" "$BUILT_FEATURES" "$PROFILE" "$UI_SCOPE" || [[ "$BUILT_FEATURES" == "missing" ]]; then
  printf 'deploy_identity_tsv\tprofile=%s\tfeatures=%s\tlocal_path=%s\tremote_path=%s\tlocal_sha256=%s\tdeployed_sha256=%s\tvalid=0\n' \
    "$PROFILE" "$BUILT_FEATURES" "$BIN" "$REMOTE" "$LOCAL_SHA256" "$REMOTE_SHA256" >&2
  echo "ERROR: deployed MagiK binary does not match the local build contract" >&2
  exit 1
fi

echo "==> Rebinding development platform manifest to deployed GUI"
verify_dev_platform_manifest rebind "$LOCAL_SHA256"
SOURCE_FIELDS="$(bench_context_source_fields "$HERE")"
printf 'deploy_identity_tsv\tprofile=%s\tfeatures=%s\tlocal_path=%s\tremote_path=%s\tlocal_sha256=%s\tdeployed_sha256=%s\tvalid=1\t%s\n' \
  "$PROFILE" "$BUILT_FEATURES" "$BIN" "$REMOTE" "$LOCAL_SHA256" "$REMOTE_SHA256" "$SOURCE_FIELDS"

echo "==> Deployed ($PROFILE)."
echo "    Main-supervised launcher was suspended and resumed when available."
echo "    Development boot: scripts/magik-mode.sh dev"
echo "    Restart only:    scripts/run-rust.sh launcher 0  (no build, no copy)"
echo "    Arcade bench:    scripts/profile-preview-scroll.sh 30 held-scroll LABEL"
echo "    Restore stock:   scripts/restore-stock-boot.sh"
