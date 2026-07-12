#!/usr/bin/env bash
# Build and atomically deploy only the catalog builder. The launcher is not restarted.
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
source "$ROOT/scripts/bench-context-lib.sh"
PROFILE=release-device
SKIP_BUILD=0
for arg in "$@"; do
  case "$arg" in
    --skip-build) SKIP_BUILD=1 ;;
    --device) ;;
    -h|--help)
      echo "usage: scripts/deploy-catalog-builder.sh [--skip-build]"
      exit 0
      ;;
    *) echo "unknown argument: $arg" >&2; exit 2 ;;
  esac
done
if [[ "$SKIP_BUILD" -eq 0 ]]; then
  "$ROOT/scripts/build-catalog-builder.sh" --device
fi
LOCAL="$ROOT/magik-gui/target/armv7-unknown-linux-gnueabihf/$PROFILE/mister-magik-catalog-builder"
REMOTE_DIR="/media/fat/mister-magik"
REMOTE="$REMOTE_DIR/mister-magik-catalog-builder"
TEMP="$REMOTE.new"
test -x "$LOCAL"
"$ROOT/scripts/mister" run "mkdir -p '$REMOTE_DIR'"
"$ROOT/scripts/mister" put "$LOCAL" "$TEMP"
"$ROOT/scripts/mister" run "chmod +x '$TEMP' && mv -f '$TEMP' '$REMOTE'"
LOCAL_SHA256="$(bench_context_sha256_file "$LOCAL")"
REMOTE_SHA256="$(bench_context_remote_sha256 "$ROOT/scripts/mister" "$REMOTE" || true)"
REMOTE_SHA256="${REMOTE_SHA256:-missing}"
if ! bench_context_require_binary_contract "$LOCAL" "$REMOTE_SHA256" builder "$PROFILE" all; then
  printf 'deploy_identity_tsv\tprofile=%s\tfeatures=builder\tlocal_path=%s\tremote_path=%s\tlocal_sha256=%s\tdeployed_sha256=%s\tvalid=0\n' \
    "$PROFILE" "$LOCAL" "$REMOTE" "$LOCAL_SHA256" "$REMOTE_SHA256" >&2
  echo "ERROR: deployed catalog builder does not match the local builder contract" >&2
  exit 1
fi
SOURCE_FIELDS="$(bench_context_source_fields "$ROOT")"
printf 'deploy_identity_tsv\tprofile=%s\tfeatures=builder\tlocal_path=%s\tremote_path=%s\tlocal_sha256=%s\tdeployed_sha256=%s\tvalid=1\t%s\n' \
  "$PROFILE" "$LOCAL" "$REMOTE" "$LOCAL_SHA256" "$REMOTE_SHA256" "$SOURCE_FIELDS"
echo "==> Deployed catalog builder without restarting the launcher: $REMOTE"
