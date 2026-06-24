#!/usr/bin/env bash
# Profile the real first-boot library scan path on a MiSTer.
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
MISTER="$HERE/scripts/mister"
REMOTE_LOG="/tmp/mister-magik-slint.log"
REMOTE_DB="/media/fat/mister-magik/library.sqlite3"
REMOTE_SUMMARY="/media/fat/mister-magik/library.summary.json"
REMOTE_ENV="/media/fat/mister-magik/launcher.env"
BENCH_DIR="$HERE/history/toolchain-bench"
TSV="$BENCH_DIR/results-first-scan.tsv"
LABEL=""
DEPLOY="skip"
REPLACE_LABEL=0
TIMEOUT_SECS=240
SQLITE_BUILD_DIR=""

usage() {
  cat <<'EOF'
Usage: scripts/profile-first-scan.sh LABEL [--deploy-device|--skip-build] [--replace-label] [--timeout SECS] [--sqlite-build-dir DIR]

Deletes the launcher catalog database and summary projection, reboots the
MiSTer, waits for the visible first-boot scan to complete, and appends timing
rows to history/toolchain-bench/results-first-scan.tsv.
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --deploy-device) DEPLOY="device"; shift ;;
    --skip-build) DEPLOY="skip"; shift ;;
    --replace-label) REPLACE_LABEL=1; shift ;;
    --timeout) TIMEOUT_SECS="${2:?}"; shift 2 ;;
    --sqlite-build-dir) SQLITE_BUILD_DIR="${2:?}"; shift 2 ;;
    --sqlite-publish-mode) echo "--sqlite-publish-mode was removed; library DB publishing has one supported path" >&2; exit 2 ;;
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
  LABEL="first-scan-$(date -u +%Y%m%dT%H%M%SZ)"
fi
if [[ ! "$LABEL" =~ ^[A-Za-z0-9_.-]+$ ]]; then
  echo "label must contain only letters, numbers, _, ., or -" >&2
  exit 2
fi
if [[ ! "$TIMEOUT_SECS" =~ ^[0-9]+$ ]]; then
  echo "--timeout must be an integer number of seconds" >&2
  exit 2
fi
mkdir -p "$BENCH_DIR"
if [[ ! -f "$TSV" ]]; then
  echo "label	commit	event	ms	notes" >"$TSV"
elif [[ "$REPLACE_LABEL" -eq 1 ]]; then
  tmp="$(mktemp)"
  { head -1 "$TSV"; grep -v "^${LABEL}	" "$TSV" | tail -n +2; } >"$tmp" || true
  mv "$tmp" "$TSV"
fi

case "$DEPLOY" in
  device) "$HERE/scripts/deploy-rust.sh" --device --ui-scope launcher ;;
  skip) : ;;
esac

commit="$(git -C "$HERE" rev-parse --short HEAD 2>/dev/null || echo unknown)"
echo "==> first-scan profile label=$LABEL commit=$commit deploy=$DEPLOY timeout=${TIMEOUT_SECS}s"
env_file="$(mktemp)"
local_log="$(mktemp)"
cleanup() {
  rm -f "$local_log" "$env_file"
  "$MISTER" run "rm -f '$REMOTE_ENV'" >/dev/null 2>&1 || true
}
trap cleanup EXIT
: >"$env_file"
if [[ -n "$SQLITE_BUILD_DIR" ]]; then
  printf 'export MISTER_LIBRARY_SQLITE_BUILD_DIR=%q\n' "$SQLITE_BUILD_DIR" >>"$env_file"
fi
printf 'export MISTER_LIBRARY_BENCH_LABEL=%q\n' "$LABEL" >>"$env_file"
printf 'export MISTER_LIBRARY_BENCH_ACTIVE_ITERATION=1\n' >>"$env_file"
"$MISTER" put "$env_file" "$REMOTE_ENV" >/dev/null
"$MISTER" run "rm -f '$REMOTE_DB' '$REMOTE_SUMMARY' '$REMOTE_LOG' /tmp/mister-magik-library-refresh.log; sync"
"$MISTER" reboot-wait

deadline=$((SECONDS + TIMEOUT_SECS))
while (( SECONDS < deadline )); do
  "$MISTER" get "$REMOTE_LOG" "$local_log" >/dev/null 2>&1 || true
  if grep -q $'^startup_timing\tlibrary_ready\t' "$local_log" || grep -q $'^startup_timing\tlibrary_db_save_failed\t' "$local_log"; then
    break
  fi
  sleep 2
done

"$MISTER" get "$REMOTE_LOG" "$local_log" >/dev/null 2>&1 || true
if grep -q $'^startup_timing\tlibrary_db_save_failed\t' "$local_log"; then
  echo "first scan failed while saving the catalog; latest log follows" >&2
  tail -80 "$local_log" >&2 || true
  exit 1
fi
ready_ms="$(awk -F '\t' '$1 == "startup_timing" && $2 == "library_ready" { ms = $3; sub(/ms$/, "", ms); print ms; exit }' "$local_log")"
if [[ -z "$ready_ms" ]]; then
  echo "first scan did not complete within ${TIMEOUT_SECS}s; latest log follows" >&2
  tail -80 "$local_log" >&2 || true
  exit 1
fi
if (( ready_ms > 60000 )); then
  echo "warning: first scan library_ready exceeded 60000ms: ${ready_ms}ms" >&2
fi

awk -v label="$LABEL" -v commit="$commit" -F '\t' '
  BEGIN { OFS = "\t" }
  $1 == "startup_timing" && ($2 == "first_frame" || $2 == "bootstrap_counter_climb" || $2 == "bootstrap_counter_sustained_climb" || $2 == "full_scan_counter_climb" || $2 == "catalog_counter_climb" || $2 == "library_scan_complete" || $2 == "library_db_saved" || $2 == "library_ready" || $2 == "catalog_bridge_sync_update" || $2 == "virtual_launch_cache_materialized") {
    ms = $3
    sub(/ms$/, "", ms)
    print label, commit, $2, ms, $4
  }
  $1 == "library_sqlite_publish_tsv" {
    print label, commit, "sqlite_publish_" $4, $11, "bytes=" $5 " copy_ms=" $7 " build_sync_ms=" $6 " final_sync_ms=" $8 " rename_ms=" $9 " parent_sync_ms=" $10 " progress_events=" $12 " result=" $13
  }
  $1 == "library_import_timing" {
    note = ($4 == "" ? "-" : $4)
    print label, commit, "import_stage_" $2, int(($3 + 500) / 1000), note
  }
  $1 == "library_scan_timing" {
    note = ($4 == "" ? "-" : $4)
    print label, commit, "scan_stage_" $2, int(($3 + 500) / 1000), note
  }
' "$local_log" >>"$TSV"

db_count="$("$MISTER" db "SELECT count(*) FROM games" 2>/dev/null | tail -1 | tr -d '\r' || true)"
status="$("$MISTER" status 2>/dev/null || true)"
printf '%s\t%s\t%s\t%s\t%s\n' "$LABEL" "$commit" "db_count" "0" "$db_count" >>"$TSV"

echo "appended to $TSV"
echo "db_count=$db_count"
printf '%s\n' "$status"
