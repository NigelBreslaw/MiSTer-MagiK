#!/usr/bin/env bash
# Capture a raw arcade-page list-scroll trace on the MiSTer and summarize it.
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
MISTER="$HERE/scripts/mister"
OUT_DIR="$HERE/build/arcade-scroll-profiles"
REMOTE="/media/fat/mister-magik/mister-magik-fb"

usage() {
  cat <<'EOF'
Usage: scripts/profile-arcade-scroll.sh [SECS] [LABEL] [--skip-build|--deploy-fast|--deploy-device]

Runs `ui arcade_page` with `MISTER_LAUNCHER_BENCH_SCENARIO=list-scroll` and
`MISTER_ARCADE_FRAME_TRACE`, pulls the raw TSV/log, then prints full/none frame
timing summaries.

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
    --deploy-fast) deploy="fast"; shift ;;
    --deploy-device) deploy="device"; shift ;;
    -h|--help) usage; exit 0 ;;
    *)
      positionals+=("$1")
      shift
      ;;
  esac
done

if [[ "${#positionals[@]}" -gt 2 ]]; then
  echo "unexpected argument: ${positionals[2]}" >&2
  usage >&2
  exit 2
fi
if [[ "${#positionals[@]}" -ge 1 ]]; then
  secs="${positionals[0]}"
fi
if [[ "${#positionals[@]}" -ge 2 ]]; then
  label="${positionals[1]}"
fi

if [[ ! "$secs" =~ ^[0-9]+$ ]]; then
  echo "secs must be an integer number of seconds" >&2
  exit 2
fi
if [[ ! "$label" =~ ^[A-Za-z0-9_.-]+$ ]]; then
  echo "label must contain only letters, numbers, _, ., or -" >&2
  exit 2
fi

mkdir -p "$OUT_DIR"
remote_tsv="/tmp/${label}-arcade-scroll.tsv"
remote_log="/tmp/${label}-arcade-scroll.log"
local_tsv="$OUT_DIR/${label}-arcade-scroll.tsv"
local_log="$OUT_DIR/${label}-arcade-scroll.log"

case "$deploy" in
  fast) "$HERE/scripts/deploy-rust.sh" --fast ;;
  device) "$HERE/scripts/deploy-rust.sh" --device ;;
  skip) ;;
esac

echo "==> Capture arcade_page list-scroll secs=$secs label=$label deploy=$deploy"
if ! "$MISTER" run "kill -9 \$(pidof mister-magik-fb) 2>/dev/null || true; kill -9 \$(pidof MiSTer_MagiK) 2>/dev/null || true; kill -9 \$(pidof MiSTer) 2>/dev/null || true; rm -f $remote_tsv $remote_log; sleep 5; MISTER_LAUNCHER_BENCH_SCENARIO=list-scroll MISTER_ARCADE_FRAME_TRACE=$remote_tsv $REMOTE ui arcade_page $secs >$remote_log 2>&1; status=\$?; grep -E 'launcher_bench_scenario|arcade_frame_trace|arcade_page fps|done:' $remote_log || true; test -s $remote_tsv || status=1; exit \$status"; then
  "$MISTER" get "$remote_log" "$local_log" || true
  echo "arcade scroll profile failed; see $local_log" >&2
  exit 1
fi

"$MISTER" get "$remote_tsv" "$local_tsv"
"$MISTER" get "$remote_log" "$local_log"
echo "wrote $local_tsv"
echo "wrote $local_log"
echo
"$HERE/scripts/analyze-arcade-frame-trace.py" "$local_tsv"
