#!/usr/bin/env bash
# Build and atomically deploy only the catalog builder. The launcher is not restarted.
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
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
echo "==> Deployed catalog builder without restarting the launcher: $REMOTE"
