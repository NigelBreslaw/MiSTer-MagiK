#!/usr/bin/env bash
# Show screenshot resize filters on the real launcher Arcade screen.
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
MISTER="$HERE/scripts/mister"
REMOTE_ENV="/media/fat/mister-magik/launcher.env"
MAX_SIZE="${1:-320x320}"
SECS="${2:-2}"
FORMAT="${3:-raw-rgb565}"

case "$MAX_SIZE" in
  *x*) ;;
  *) echo "usage: scripts/preview-resize-demo.sh [MAX_SIZE=320x320] [SECS=2] [FORMAT=raw-rgb565]" >&2; exit 2 ;;
esac
case "$FORMAT" in
  raw-rgb565|raw565|rgb565|565) ;;
  *) echo "usage: scripts/preview-resize-demo.sh [MAX_SIZE=320x320] [SECS=2] [FORMAT=raw-rgb565]" >&2; exit 2 ;;
esac
if [[ ! "$SECS" =~ ^[0-9]+$ ]]; then
  echo "SECS must be an integer" >&2
  exit 2
fi

env_file="$(mktemp)"
cleanup() {
  rm -f "$env_file"
  "$MISTER" run "rm -f '$REMOTE_ENV'; if [ -p /dev/MiSTer_cmd ]; then printf 'mister_magik_restart_launcher\n' > /dev/MiSTer_cmd; fi" >/dev/null 2>&1 || true
}
trap cleanup EXIT

for filter in nearest box lanczos hybrid; do
  echo "preview_resize_demo filter=$filter max=$MAX_SIZE"
  {
    printf 'export MISTER_FB_FORMAT=565\n'
    printf 'export MISTER_CATALOG_REFRESH=off\n'
    printf 'export MISTER_MAGIK_LIBRARY_REFRESH_DELAY_SECS=9999\n'
    printf 'export MISTER_LAUNCHER_START_SCREEN=arcade\n'
    printf 'export MISTER_LAUNCHER_LOCK_SCREEN=arcade\n'
    printf 'export MISTER_LAUNCHER_BENCH_SCENARIO=preview-step-hold\n'
    printf 'export MISTER_PREVIEW_RESIZE_FILTER=%q\n' "$filter"
    printf 'export MISTER_PREVIEW_RESIZE_MAX=%q\n' "$MAX_SIZE"
    printf 'export MISTER_PREVIEW_FORMAT=%q\n' "$FORMAT"
    printf 'export MISTER_PREVIEW_RUN_LABEL=%q\n' "$filter resize - $FORMAT - $MAX_SIZE"
  } >"$env_file"
  "$MISTER" put "$env_file" "$REMOTE_ENV" >/dev/null
  "$MISTER" run "if [ ! -p /dev/MiSTer_cmd ]; then echo 'missing /dev/MiSTer_cmd'; exit 12; fi; printf 'mister_magik_restart_launcher\n' > /dev/MiSTer_cmd" >/dev/null
  sleep "$SECS"
done
