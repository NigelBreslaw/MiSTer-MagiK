#!/usr/bin/env bash
# Run MiSTer library SQLite publish/save benchmarks and append publish rows.
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
MISTER="$HERE/scripts/mister"
BENCH_DIR="$HERE/history/toolchain-bench"
TSV="$BENCH_DIR/results-library-save.tsv"

LABEL=""
ITERATIONS=1
REPLACE_LABEL=0
REMOTE_BIN="${MISTER_MAGIK_REMOTE_BIN:-/media/fat/mister-magik/mister-magik-fb}"
SQLITE_BASE="/media/fat/mister-magik/library-save-bench"

usage() {
  cat <<'EOF'
Usage: scripts/profile-library-save.sh LABEL [--iterations N] [--replace-label]

Runs fresh library-refresh passes on the MiSTer and captures the final SQLite
publish timing. The rows isolate publishing the completed database to /media/fat
from discovery and SQLite import work.
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --iterations) ITERATIONS="${2:?}"; shift 2 ;;
    --modes) echo "--modes was removed; library DB publishing has one progress-capable path" >&2; exit 2 ;;
    --replace-label) REPLACE_LABEL=1; shift ;;
    -h|--help) usage; exit 0 ;;
    --*) echo "unknown option: $1" >&2; usage >&2; exit 2 ;;
    *)
      if [[ -n "$LABEL" ]]; then
        echo "unexpected argument: $1" >&2
        usage >&2
        exit 2
      fi
      LABEL="$1"
      shift
      ;;
  esac
done

if [[ -z "$LABEL" ]]; then
  LABEL="library-save-$(date -u +%Y%m%dT%H%M%SZ)"
fi
if [[ ! "$LABEL" =~ ^[A-Za-z0-9_.-]+$ ]]; then
  echo "label must contain only letters, numbers, _, ., or -" >&2
  exit 2
fi
if [[ ! "$ITERATIONS" =~ ^[0-9]+$ || "$ITERATIONS" -lt 1 ]]; then
  echo "iterations must be a positive integer" >&2
  exit 2
fi

mkdir -p "$BENCH_DIR"
if [[ "$REPLACE_LABEL" -eq 1 && -f "$TSV" ]]; then
  tmp="$(mktemp)"
  { head -1 "$TSV"; grep -v "^library_sqlite_publish_tsv	${LABEL}	" "$TSV" | tail -n +2; } >"$tmp" || true
  mv "$tmp" "$TSV"
fi
if [[ ! -f "$TSV" ]]; then
  printf 'type\tlabel\titeration\tmode\tbytes\tbuild_sync_ms\tcopy_ms\tfinal_sync_ms\trename_ms\tparent_sync_ms\ttotal_ms\tprogress_events\tresult\n' >"$TSV"
fi

shell_quote() {
  printf "'%s'" "${1//\'/\'\\\'\'}"
}

remote_run() {
  "$MISTER" run "$1"
}

magik_command() {
  remote_run "if [ -p /dev/MiSTer_cmd ] && pidof MiSTer_MagiK >/dev/null 2>&1; then printf '$1\n' > /dev/MiSTer_cmd; fi" >/dev/null 2>&1 || true
}

run_with_launcher_suspended() {
  trap 'magik_command "mister_magik_resume"' RETURN
  magik_command "mister_magik_suspend"
  remote_run "$1"
  magik_command "mister_magik_resume"
  trap - RETURN
}

echo "== library SQLite save iterations=$ITERATIONS =="
for ((iteration = 1; iteration <= ITERATIONS; iteration++)); do
  sqlite_path="${SQLITE_BASE}-${iteration}.sqlite3"
  remote_cmd="rm -f $(shell_quote "$sqlite_path") $(shell_quote "/tmp/mister-magik/library-refresh.lock")"
  remote_cmd+="; MISTER_LIBRARY_BENCH_LABEL=$(shell_quote "$LABEL")"
  remote_cmd+=" MISTER_LIBRARY_BENCH_ACTIVE_ITERATION=$(shell_quote "$iteration")"
  remote_cmd+=" MISTER_LIBRARY_SQLITE=$(shell_quote "$sqlite_path")"
  remote_cmd+=" $(shell_quote "$REMOTE_BIN") library-refresh"

  out="$(run_with_launcher_suspended "$remote_cmd")"
  printf '%s\n' "$out"
  printf '%s\n' "$out" | grep '^library_sqlite_publish_tsv	' >>"$TSV" || true
done

echo "appended to $TSV"
