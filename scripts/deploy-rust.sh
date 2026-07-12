#!/usr/bin/env bash
# Cross-build the Rust frontend and deploy the binary to the MiSTer.
#
# This is a production-safe file deploy: when Main_MiSTer supervises the
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
source "$HERE/scripts/bench-context-lib.sh"
REMOTE_DIR="/media/fat/mister-magik"
REMOTE="$REMOTE_DIR/mister-magik-fb"
REMOTE_ART_DIR="$REMOTE_DIR/art"
DEPLOY_TRANSPORT="${MISTER_DEPLOY_TRANSPORT:-agent}"
LOCAL_SLINT_LOGO="$HERE/magik-gui/ui/art/slint-logo-pixel.png"

PROFILE=release-device
BUILD_FLAG=(--device)
UI_SCOPE="${MISTER_UI_BUILD_SCOPE:-all}"
ARGS=("$@")
for ((i = 0; i < ${#ARGS[@]}; i++)); do
  arg="${ARGS[$i]}"
  case "$arg" in
    --device) PROFILE=release-device; BUILD_FLAG=(--device) ;;
    --video|--video-lab|--mame-metadata|--hbmame-metadata|--asset-packs)
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

echo "==> Cross-building (armv7 profile=$PROFILE)"
"$HERE/magik-gui/build-arm.sh" "${BUILD_FLAG[@]}"

LOCAL_BYTES="$(bytes "$BIN")"
echo "==> Local binary size: $LOCAL_BYTES bytes ($(human_bytes "$LOCAL_BYTES"))"

echo "==> Deploying Slint logo -> $REMOTE_ART_DIR"
LOCAL_SLINT_LOGO_RAW="$(mktemp "${TMPDIR:-/tmp}/slint-logo-pixel-rgba.XXXXXX")"
python3 "$HERE/scripts/png-to-slint-rgba.py" "$LOCAL_SLINT_LOGO" "$LOCAL_SLINT_LOGO_RAW"
MISTER_IP="${MISTER_IP:-192.168.1.117}" \
MISTER_PASS="${MISTER_PASS:-1}" \
  "$HERE/scripts/mister" run "mkdir -p '$REMOTE_ART_DIR'" >/dev/null
MISTER_IP="${MISTER_IP:-192.168.1.117}" \
MISTER_PASS="${MISTER_PASS:-1}" \
  "$HERE/scripts/mister" put "$LOCAL_SLINT_LOGO_RAW" "$REMOTE_ART_DIR/slint-logo-pixel.rgba" >/dev/null
rm -f "$LOCAL_SLINT_LOGO_RAW"

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
LOCAL_SHA256="$(bench_context_sha256_file "$BIN")"
REMOTE_SHA256="$(bench_context_remote_sha256 "$HERE/scripts/mister" "$REMOTE" || true)"
REMOTE_SHA256="${REMOTE_SHA256:-missing}"
BUILT_FEATURES="$(bench_context_binary_features "$BIN")"
if ! bench_context_require_binary_contract "$BIN" "$REMOTE_SHA256" "$BUILT_FEATURES" "$PROFILE" "$UI_SCOPE" || [[ "$BUILT_FEATURES" == "missing" ]]; then
  printf 'deploy_identity_tsv\tprofile=%s\tfeatures=%s\tlocal_path=%s\tremote_path=%s\tlocal_sha256=%s\tdeployed_sha256=%s\tvalid=0\n' \
    "$PROFILE" "$BUILT_FEATURES" "$BIN" "$REMOTE" "$LOCAL_SHA256" "$REMOTE_SHA256" >&2
  echo "ERROR: deployed MagiK binary does not match the local build contract" >&2
  exit 1
fi
SOURCE_FIELDS="$(bench_context_source_fields "$HERE")"
printf 'deploy_identity_tsv\tprofile=%s\tfeatures=%s\tlocal_path=%s\tremote_path=%s\tlocal_sha256=%s\tdeployed_sha256=%s\tvalid=1\t%s\n' \
  "$PROFILE" "$BUILT_FEATURES" "$BIN" "$REMOTE" "$LOCAL_SHA256" "$REMOTE_SHA256" "$SOURCE_FIELDS"

echo "==> Deployed ($PROFILE)."
echo "==> Building and deploying catalog builder"
"$HERE/scripts/deploy-catalog-builder.sh"
echo "    Main-supervised launcher was suspended and resumed when available."
echo "    Production boot: scripts/install-slint-boot.sh  (once — MiSTer.ini main= handoff)"
echo "    Restart only:    scripts/run-rust.sh launcher 0  (no build, no copy)"
echo "    Arcade bench:    scripts/profile-preview-scroll.sh 30 held-scroll LABEL"
echo "    Restore stock:   scripts/restore-stock-boot.sh"
