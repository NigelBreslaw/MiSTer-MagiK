#!/usr/bin/env bash
# Cross-build the Rust frontend and deploy the binary to the MiSTer.
#
# This is a production-safe file deploy: when Main_MiSTer supervises the
# launcher, deploy asks it to suspend MagiK, swaps the binary, then resumes.
#
#   MISTER_IP=192.168.1.117 MISTER_PASS=1 scripts/deploy-rust.sh
#   MISTER_IP=... scripts/deploy-rust.sh --all-scenes
#   MISTER_IP=... scripts/deploy-rust.sh --experiments
#   MISTER_IP=... scripts/deploy-rust.sh --ui-scope launcher
#   MISTER_DEPLOY_TRANSPORT=ssh scripts/deploy-rust.sh  # explicit fallback only
#
# Default installs the release-device (A3) binary.
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
REMOTE_DIR="/media/fat/mister-magik"
REMOTE="$REMOTE_DIR/mister-magik-fb"
DEPLOY_TRANSPORT="${MISTER_DEPLOY_TRANSPORT:-agent}"

PROFILE=release-device
BUILD_FLAG=(--device)
ARGS=("$@")
for ((i = 0; i < ${#ARGS[@]}; i++)); do
  arg="${ARGS[$i]}"
  case "$arg" in
    --device) PROFILE=release-device; BUILD_FLAG=(--device) ;;
    --video|--mame-metadata|--hbmame-metadata|--asset-packs)
      echo "ERROR: $arg was removed from scripts/deploy-rust.sh; deploy runtime here and use catalog/media build tools explicitly" >&2
      exit 2
      ;;
    --all-scenes) BUILD_FLAG+=(--all-scenes) ;;
    --experiments) BUILD_FLAG+=(--experiments) ;;
    --ui-scope=*) BUILD_FLAG+=("$arg") ;;
    --ui-scope)
      i=$((i + 1))
      if [ "$i" -ge "${#ARGS[@]}" ]; then
        echo "ERROR: --ui-scope requires one of: launcher, arcade, all" >&2
        exit 2
      fi
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

echo "==> Deployed ($PROFILE)."
echo "    Main-supervised launcher was suspended and resumed when available."
echo "    Production boot: scripts/install-slint-boot.sh  (once — MiSTer.ini main= handoff)"
echo "    Restart only:    scripts/run-rust.sh launcher 0  (no build, no copy)"
echo "    Arcade bench:    scripts/profile-preview-scroll.sh 30 held-scroll LABEL"
echo "    Restore stock:   scripts/restore-stock-boot.sh"
