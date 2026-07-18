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
#   MISTER_IP=... scripts/deploy-rust.sh --fast           # optimized thin-LTO daily build
#   MISTER_IP=... scripts/deploy-rust.sh --ui-scope launcher
#   MISTER_DEPLOY_TRANSPORT=ssh scripts/deploy-rust.sh  # explicit fallback only
#
# Default installs the release-device (A3) binary.
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
source "$HERE/scripts/lib/bench-context-lib.sh"
source "$HERE/scripts/lib/magik-layout.sh"
source "$HERE/scripts/lib/platform-manifest-lib.sh"
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
UI_SCOPE_EXPLICIT=0
[ -n "${MISTER_UI_BUILD_SCOPE:-}" ] && UI_SCOPE_EXPLICIT=1
ARGS=("$@")
for ((i = 0; i < ${#ARGS[@]}; i++)); do
  arg="${ARGS[$i]}"
  case "$arg" in
    --device) PROFILE=release-device; BUILD_FLAG=(--device) ;;
    --fast) PROFILE=release; BUILD_FLAG=(--fast) ;;
    --video|--mame-metadata|--hbmame-metadata|--asset-packs)
      echo "ERROR: $arg was removed from scripts/deploy-rust.sh; deploy runtime here and use catalog/media build tools explicitly" >&2
      exit 2
      ;;
    --all-scenes) UI_SCOPE=all; UI_SCOPE_EXPLICIT=1; BUILD_FLAG+=(--all-scenes) ;;
    --experiments) UI_SCOPE=all; UI_SCOPE_EXPLICIT=1; BUILD_FLAG+=(--experiments) ;;
    --bench-tools) BUILD_FLAG+=(--bench-tools) ;;
    --diagnostics) BUILD_FLAG+=(--diagnostics) ;;
    --ui-scope=*) UI_SCOPE="${arg#--ui-scope=}"; UI_SCOPE_EXPLICIT=1; BUILD_FLAG+=("$arg") ;;
    --ui-scope)
      i=$((i + 1))
      if [ "$i" -ge "${#ARGS[@]}" ]; then
        echo "ERROR: --ui-scope requires one of: launcher, arcade, all" >&2
        exit 2
      fi
      UI_SCOPE="${ARGS[$i]}"
      UI_SCOPE_EXPLICIT=1
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

if [ "$PROFILE" = release ] && [ "$UI_SCOPE_EXPLICIT" -eq 0 ]; then
  UI_SCOPE=launcher
fi

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
  local candidate_magik_revision="${3:-}"
  platform_manifest_verify "$HERE/scripts/mister" dev \
    "$MISTER_MAGIK_MANIFEST" "" "$mode" "$candidate_gui_sha" "$candidate_magik_revision"
}

echo "==> Cross-building (armv7 profile=$PROFILE)"
"$HERE/magik-gui/build-arm.sh" "${BUILD_FLAG[@]}"

LOCAL_BYTES="$(bytes "$BIN")"
LOCAL_SHA256="$(bench_context_sha256_file "$BIN")"
echo "==> Local binary size: $LOCAL_BYTES bytes ($(human_bytes "$LOCAL_BYTES"))"

echo "==> Preflighting development platform manifest"
verify_dev_platform_manifest verify-platform

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

# scripts/mister makes canonical GUI deployment, manifest rebinding, and latch
# activation one fail-closed operation. Verify again here because this is the
# public high-level deploy contract.
echo "==> Verifying rebound development platform manifest and active latch path"
verify_dev_platform_manifest verify
SOURCE_FIELDS="$(bench_context_source_fields "$HERE")"
printf 'deploy_identity_tsv\tprofile=%s\tfeatures=%s\tlocal_path=%s\tremote_path=%s\tlocal_sha256=%s\tdeployed_sha256=%s\tvalid=1\t%s\n' \
  "$PROFILE" "$BUILT_FEATURES" "$BIN" "$REMOTE" "$LOCAL_SHA256" "$REMOTE_SHA256" "$SOURCE_FIELDS"

echo "==> Deployed ($PROFILE)."
echo "    Main-supervised launcher was suspended and resumed when available."
echo "    Development boot: scripts/magik-mode.sh dev"
echo "    Restart only:    scripts/run-rust.sh launcher 0  (no build, no copy)"
echo "    Arcade bench:    scripts/profile-preview-scroll.sh 30 held-scroll LABEL"
echo "    Restore stock:   scripts/restore-stock-boot.sh"
