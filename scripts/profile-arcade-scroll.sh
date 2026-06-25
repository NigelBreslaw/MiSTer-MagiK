#!/usr/bin/env bash
# Capture a real launcher Arcade velocity-scroll trace through MiSTer_MagiK.
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
MISTER="$HERE/scripts/mister"
OUT_DIR="$HERE/build/arcade-scroll-profiles"

usage() {
  cat <<'EOF'
Usage: scripts/profile-arcade-scroll.sh [SECS] [LABEL] [--skip-build|--deploy-device]

Runs the Main-supervised launcher on the real Arcade screen with
MISTER_LAUNCHER_BENCH_SCENARIO=held-scroll and MISTER_PREVIEW_SCROLL_TRACE,
pulls the raw TSV/log, then prints frame timing summaries.

Do not use row-step `list-scroll` for arcade performance benchmarking. It does
not reproduce real velocity scrolling.

Default: --skip-build, useful when the desired binary is already deployed.
EOF
}

secs="15"
label="arcade-scroll-$(date -u +%Y%m%dT%H%M%SZ)"
deploy="skip"
positionals=()

while [[ $# -gt 0 ]]; do
  case "$1" in
    --skip-build) deploy="skip"; shift ;;
    --deploy-device) deploy="device"; shift ;;
    -h|--help) usage; exit 0 ;;
    *) positionals+=("$1"); shift ;;
  esac
done

if [[ "${#positionals[@]}" -gt 2 ]]; then
  echo "unexpected argument: ${positionals[2]}" >&2
  usage >&2
  exit 2
fi
if [[ "${#positionals[@]}" -ge 1 ]]; then secs="${positionals[0]}"; fi
if [[ "${#positionals[@]}" -ge 2 ]]; then label="${positionals[1]}"; fi

if [[ ! "$secs" =~ ^[0-9]+$ ]]; then echo "secs must be an integer number of seconds" >&2; exit 2; fi
if [[ ! "$label" =~ ^[A-Za-z0-9_.-]+$ ]]; then echo "label must contain only letters, numbers, _, ., or -" >&2; exit 2; fi

mkdir -p "$OUT_DIR"
remote_tsv="/tmp/${label}-arcade-scroll.tsv"
remote_log="/tmp/mister-magik-slint.log"
local_tsv="$OUT_DIR/${label}-arcade-scroll.tsv"
local_log="$OUT_DIR/${label}-arcade-scroll.log"

cleanup() {
  "$MISTER" launcher-restart --clear-env --timeout 20 >/dev/null 2>&1 || true
}
trap cleanup EXIT

case "$deploy" in
  device) "$HERE/scripts/deploy-rust.sh" --device --ui-scope launcher ;;
  skip) : ;;
esac

echo "==> Capture supervised launcher Arcade held-scroll secs=$secs label=$label deploy=$deploy"
"$MISTER" run "rm -f '$remote_tsv' '$remote_log'" >/dev/null
"$MISTER" launcher-restart \
  --env MISTER_CATALOG_REFRESH=default \
  --env MISTER_LAUNCHER_START_SCREEN=arcade \
  --env MISTER_LAUNCHER_LOCK_SCREEN=arcade \
  --env MISTER_LAUNCHER_BENCH_SCENARIO=held-scroll \
  --env MISTER_PREVIEW_TRACE=1 \
  --env "MISTER_PREVIEW_SCROLL_TRACE_SECS=$secs" \
  --env "MISTER_PREVIEW_SCROLL_TRACE=$remote_tsv" \
  --timeout 30 >/dev/null
sleep $((secs + 7))

if ! "$MISTER" get "$remote_tsv" "$local_tsv" >/dev/null; then
  "$MISTER" get "$remote_log" "$local_log" >/dev/null || true
  echo "arcade scroll profile failed; see $local_log" >&2
  exit 1
fi
"$MISTER" get "$remote_log" "$local_log" >/dev/null || true

echo "wrote $local_tsv"
echo "wrote $local_log"
echo
"$HERE/scripts/analyze-arcade-frame-trace.py" "$local_tsv"
echo
"$HERE/scripts/launcher-present-trace.py" summarize "$local_tsv" --case arcade-scroll
