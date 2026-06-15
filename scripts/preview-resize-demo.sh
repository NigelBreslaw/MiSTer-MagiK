#!/usr/bin/env bash
# Show screenshot resize filters on the MiSTer display, 2 seconds each.
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
MISTER="$HERE/scripts/mister"
REMOTE="/media/fat/mister-magik/mister-magik-fb"
MAX_SIZE="${1:-320x320}"
SECS="${2:-2}"
FORMAT="${3:-derived-png}"
LOG="/tmp/mister-magik-preview-resize-demo.log"

case "$MAX_SIZE" in
  *x*) ;;
  *) echo "usage: scripts/preview-resize-demo.sh [MAX_SIZE=320x320] [SECS=2]" >&2; exit 2 ;;
esac
case "$FORMAT" in
  png|derived-png|raw-rgb) ;;
  *) echo "usage: scripts/preview-resize-demo.sh [MAX_SIZE=320x320] [SECS=2] [FORMAT=derived-png]" >&2; exit 2 ;;
esac

"$MISTER" run "
set -e
kill -9 \$(pidof mister-magik-fb) 2>/dev/null || true
kill -9 \$(pidof MiSTer_MagiK) 2>/dev/null || true
kill -9 \$(pidof MiSTer) 2>/dev/null || true
REMOTE='$REMOTE'
test -x \$REMOTE || chmod +x \$REMOTE
for FILTER in nearest box lanczos hybrid; do
  echo preview_resize_demo filter=\$FILTER max='$MAX_SIZE'
  MISTER_CATALOG_REFRESH=off \
  MISTER_LAUNCHER_BENCH_SCENARIO=preview-step-hold \
  MISTER_PREVIEW_RESIZE_FILTER=\$FILTER \
  MISTER_PREVIEW_RESIZE_MAX='$MAX_SIZE' \
  MISTER_PREVIEW_FORMAT='$FORMAT' \
  MISTER_PREVIEW_RUN_LABEL=\"\$FILTER resize - $FORMAT - $MAX_SIZE\" \
  \$REMOTE ui arcade '$SECS' >'$LOG' 2>&1 || exit \$?
done
"
