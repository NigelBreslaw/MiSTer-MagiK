#!/usr/bin/env bash
# Profile the real first-boot library scan path on a MiSTer.
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
MISTER="$HERE/scripts/mister"
REMOTE_LOG="/tmp/mister-magik-slint.log"
REMOTE_REFRESH_LOG="/tmp/mister-magik-library-refresh.log"
REMOTE_DB="/media/fat/mister-magik/library.sqlite3"
REMOTE_SUMMARY="/media/fat/mister-magik/library.summary.json"
REMOTE_NAV="/media/fat/mister-magik/library.nav.lz4b"
REMOTE_ENV="/media/fat/mister-magik/launcher.env"
BENCH_DIR="$HERE/history/toolchain-bench"
OUT_DIR="$HERE/build/first-scan-profiles"
TSV="$BENCH_DIR/results-first-scan.tsv"
LABEL=""
DEPLOY="skip"
REPLACE_LABEL=0
TIMEOUT_SECS=240
SQLITE_BUILD_DIR=""
RAM_CATALOG_READY_GATE_MS=95467
DB_SAVE_GATE_MS=121426
source "$HERE/scripts/thread-sampler-lib.sh"
source "$HERE/scripts/mister-supervision-lib.sh"

usage() {
  cat <<'EOF'
Usage: scripts/profile-first-scan.sh LABEL [--deploy-device|--deploy-catalog|--skip-build] [--replace-label] [--timeout SECS] [--sqlite-build-dir DIR] [--thread-sample]
       scripts/profile-first-scan.sh --self-test

Deletes the launcher catalog database and summary projection, reboots the
MiSTer, waits for the visible first-boot scan to complete, and appends timing
rows to history/toolchain-bench/results-first-scan.tsv.
--thread-sample records /proc per-thread CPU/core/scheduler samples once per
second after reboot while the first scan completes.
EOF
}

first_scan_gate_check() {
  local ready_ms="$1"
  local saved_ms="$2"
  if (( ready_ms > RAM_CATALOG_READY_GATE_MS )); then
    return 1
  fi
  if (( saved_ms > DB_SAVE_GATE_MS )); then
    return 1
  fi
  return 0
}

first_scan_commit_is_dirty_from_statuses() {
  local worktree_status="$1"
  local index_status="$2"
  [[ "$worktree_status" -ne 0 || "$index_status" -ne 0 ]]
}

first_scan_commit_is_dirty() {
  local repo="$1"
  local worktree_status index_status
  if git -C "$repo" diff --quiet -- . ':!history/toolchain-bench/results-first-scan.tsv'; then
    worktree_status=0
  else
    worktree_status=$?
  fi
  if git -C "$repo" diff --cached --quiet -- . ':!history/toolchain-bench/results-first-scan.tsv'; then
    index_status=0
  else
    index_status=$?
  fi
  first_scan_commit_is_dirty_from_statuses "$worktree_status" "$index_status"
}

first_scan_self_test() {
  first_scan_gate_check 56094 71573
  first_scan_gate_check "$RAM_CATALOG_READY_GATE_MS" "$DB_SAVE_GATE_MS"
  if first_scan_gate_check $((RAM_CATALOG_READY_GATE_MS + 1)) "$DB_SAVE_GATE_MS"; then
    echo "ready gate accepted gate+1" >&2
    return 1
  fi
  if first_scan_gate_check "$RAM_CATALOG_READY_GATE_MS" $((DB_SAVE_GATE_MS + 1)); then
    echo "save gate accepted gate+1" >&2
    return 1
  fi
  if first_scan_commit_is_dirty_from_statuses 0 0; then
    echo "first-scan dirty helper marked a clean source dirty" >&2
    return 1
  fi
  if ! first_scan_commit_is_dirty_from_statuses 1 0; then
    echo "first-scan dirty helper ignored an unstaged source diff" >&2
    return 1
  fi
  if ! first_scan_commit_is_dirty_from_statuses 0 1; then
    echo "first-scan dirty helper ignored a staged source diff" >&2
    return 1
  fi
  first_scan_reset_artifact_self_test
  echo "profile-first-scan self-test ok"
}

first_scan_reset_artifact_self_test() {
  local rows
  rows="$(
    for path in "$REMOTE_DB" "$REMOTE_SUMMARY" "$REMOTE_NAV"; do
      printf 'artifact_reset_tsv\tSELFTEST\tmissing\t%s\t0\n' "$path"
    done
  )"
  if [[ "$(printf '%s\n' "$rows" | wc -l | tr -d ' ')" != "3" ]]; then
    echo "artifact reset self-test expected three rows" >&2
    return 1
  fi
  for path in "$REMOTE_DB" "$REMOTE_SUMMARY" "$REMOTE_NAV"; do
    if ! printf '%s\n' "$rows" | grep -q $'^artifact_reset_tsv\tSELFTEST\tmissing\t'"$path"$'\t0$'; then
      echo "artifact reset self-test missing row for $path" >&2
      return 1
    fi
  done
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --self-test) first_scan_self_test; exit 0 ;;
    --deploy-device) DEPLOY="device"; shift ;;
    --deploy-catalog) DEPLOY="catalog"; shift ;;
    --skip-build) DEPLOY="skip"; shift ;;
    --replace-label) REPLACE_LABEL=1; shift ;;
    --timeout) TIMEOUT_SECS="${2:?}"; shift 2 ;;
    --sqlite-build-dir) SQLITE_BUILD_DIR="${2:?}"; shift 2 ;;
    --thread-sample) thread_sample_enabled="1"; shift ;;
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
label="$LABEL"
mkdir -p "$BENCH_DIR" "$OUT_DIR"
if [[ ! -f "$TSV" ]]; then
  echo "label	commit	event	ms	notes" >"$TSV"
elif [[ "$REPLACE_LABEL" -eq 1 ]]; then
  tmp="$(mktemp)"
  { head -1 "$TSV"; grep -v "^${LABEL}	" "$TSV" | tail -n +2; } >"$tmp" || true
  mv "$tmp" "$TSV"
fi

case "$DEPLOY" in
  device) "$HERE/scripts/deploy-rust.sh" --device --ui-scope launcher ;;
  catalog) "$HERE/scripts/deploy-catalog-builder.sh" ;;
  skip) : ;;
esac

ensure_launcher_recovered() {
  local phase="$1"
  local status
  status="$("$MISTER" run "cat /tmp/mister-magik/main-status.json 2>/dev/null || true" 2>/dev/null || true)"
  if printf '%s\n' "$status" | grep -q '"launcher_state"[[:space:]]*:[[:space:]]*"LauncherCrashed"'; then
    echo "==> launcher is crashed before first-scan $phase; restarting supervised launcher"
    "$MISTER" agent magik restart-launcher >/dev/null
    status="$("$MISTER" run "cat /tmp/mister-magik/main-status.json 2>/dev/null || true" 2>/dev/null || true)"
  fi
  if printf '%s\n' "$status" | grep -q '"launcher_state"[[:space:]]*:[[:space:]]*"LauncherCrashed"'; then
    echo "first-scan $phase cannot continue: launcher remains LauncherCrashed" >&2
    printf '%s\n' "$status" >&2
    exit 1
  fi
}

ensure_launcher_recovered "setup"

commit="$(git -C "$HERE" rev-parse --short HEAD 2>/dev/null || echo unknown)"
if [[ "$commit" != "unknown" ]] && first_scan_commit_is_dirty "$HERE"; then
  commit="${commit}-dirty"
fi
echo "==> first-scan profile label=$LABEL commit=$commit deploy=$DEPLOY timeout=${TIMEOUT_SECS}s"
env_file="$(mktemp)"
local_log="$(mktemp)"
local_refresh_log="$(mktemp)"
combined_log="$(mktemp)"
raw_log="$OUT_DIR/${LABEL}-launcher.log"
raw_refresh_log="$OUT_DIR/${LABEL}-catalog-builder.log"
artifact_report="$OUT_DIR/${LABEL}-artifacts.tsv"
launcher_suspended=0
emit_thread_sample_artifact_report() {
  local raw_log_bytes=0
  if [[ -f "$raw_log" ]]; then
    raw_log_bytes="$(wc -c <"$raw_log" | tr -d ' ')"
  fi
  printf 'artifact_tsv\tlabel=%s\tkind=launcher_log\tlocal_path=%s\tremote_path=%s\texists=%s\tbytes=%s\n' \
    "$LABEL" "$raw_log" "$REMOTE_LOG" "$([[ -f "$raw_log" ]] && echo true || echo false)" "$raw_log_bytes"
  local raw_refresh_log_bytes=0
  if [[ -f "$raw_refresh_log" ]]; then
    raw_refresh_log_bytes="$(wc -c <"$raw_refresh_log" | tr -d ' ')"
  fi
  printf 'artifact_tsv\tlabel=%s\tkind=catalog_builder_log\tlocal_path=%s\tremote_path=%s\texists=%s\tbytes=%s\n' \
    "$LABEL" "$raw_refresh_log" "$REMOTE_REFRESH_LOG" "$([[ -f "$raw_refresh_log" ]] && echo true || echo false)" "$raw_refresh_log_bytes"
  if [[ "$thread_sample_enabled" == "1" ]]; then
    thread_sample_emit_artifacts
  fi
}
cleanup() {
  rm -f "$local_log" "$local_refresh_log" "$combined_log" "$env_file"
  "$MISTER" run "rm -f '$REMOTE_ENV'" >/dev/null 2>&1 || true
  if [[ "$launcher_suspended" == "1" ]]; then
    mister_supervision_command "mister_magik_resume" 0.5 >/dev/null 2>&1 || true
  fi
}
trap cleanup EXIT
: >"$env_file"
if [[ -n "$SQLITE_BUILD_DIR" ]]; then
  printf 'export MISTER_LIBRARY_SQLITE_BUILD_DIR=%q\n' "$SQLITE_BUILD_DIR" >>"$env_file"
fi
printf 'export MISTER_LIBRARY_BENCH_LABEL=%q\n' "$LABEL" >>"$env_file"
printf 'export MISTER_LIBRARY_BENCH_ACTIVE_ITERATION=1\n' >>"$env_file"
"$MISTER" put "$env_file" "$REMOTE_ENV" >/dev/null
echo "==> Quiescing launcher and standalone catalog builder before artifact reset"
mister_suspend_launcher 1 >/dev/null
launcher_suspended=1
reset_report="$("$MISTER" run "
builder_pids=\$(pidof mister-magik-catalog-builder 2>/dev/null || true)
if [ -n \"\$builder_pids\" ]; then
  kill \$builder_pids 2>/dev/null || true
  attempts=0
  while pidof mister-magik-catalog-builder >/dev/null 2>&1 && [ \$attempts -lt 20 ]; do
    sleep 0.1
    attempts=\$((attempts + 1))
  done
  builder_pids=\$(pidof mister-magik-catalog-builder 2>/dev/null || true)
  if [ -n \"\$builder_pids\" ]; then
    kill -9 \$builder_pids 2>/dev/null || true
  fi
fi
rm -f /tmp/mister-magik/catalog-builder.lock
for path in '$REMOTE_DB' '$REMOTE_SUMMARY' '$REMOTE_NAV'; do
  if [ -e \"\$path\" ]; then
    bytes=\$(wc -c <\"\$path\" 2>/dev/null || echo 0)
    echo \"artifact_reset_tsv	$LABEL	removed	\$path	\$bytes\"
  else
    echo \"artifact_reset_tsv	$LABEL	missing	\$path	0\"
  fi
done
rm -f '$REMOTE_DB' '$REMOTE_SUMMARY' '$REMOTE_NAV' '$REMOTE_LOG' '$REMOTE_REFRESH_LOG'
sync
for path in '$REMOTE_DB' '$REMOTE_SUMMARY' '$REMOTE_NAV'; do
  if [ -e \"\$path\" ]; then
    echo \"artifact reset failed: \$path was republished\" >&2
    exit 1
  fi
done
sleep 1
for path in '$REMOTE_DB' '$REMOTE_SUMMARY' '$REMOTE_NAV'; do
  if [ -e \"\$path\" ]; then
    echo \"artifact reset failed after settle: \$path was republished\" >&2
    exit 1
  fi
done
")"
printf '%s\n' "$reset_report" | tee "$OUT_DIR/${LABEL}-artifact-reset.tsv"
## Main does not accept reboot requests while its launcher is suspended. Resume
## without a settle delay, then immediately request the supervised reboot.
mister_supervision_command "mister_magik_resume" 0 >/dev/null
launcher_suspended=0
"$MISTER" reboot-wait
thread_sample_start "$LABEL" "first-scan" "$OUT_DIR" "$TIMEOUT_SECS"

deadline=$((SECONDS + TIMEOUT_SECS))
while (( SECONDS < deadline )); do
  "$MISTER" get "$REMOTE_REFRESH_LOG" "$local_refresh_log" >/dev/null 2>&1 || true
  "$MISTER" get "$REMOTE_LOG" "$local_log" >/dev/null 2>&1 || true
  if grep -q '"event":"failure"' "$local_refresh_log" ||
     grep -q $'^startup_timing\tlibrary_db_save_failed\t' "$local_log"; then
    break
  fi
  if grep -q '"name":"builder_persisted"' "$local_refresh_log" &&
     grep -q $'^startup_timing\tfirst_frame\t' "$local_log"; then
    break
  fi
  sleep 2
done

"$MISTER" get "$REMOTE_REFRESH_LOG" "$local_refresh_log" >/dev/null 2>&1 || true
"$MISTER" get "$REMOTE_LOG" "$local_log" >/dev/null 2>&1 || true
cp "$local_log" "$raw_log"
cp "$local_refresh_log" "$raw_refresh_log"
thread_sample_stop
thread_sample_collect
if grep -q '"event":"failure"' "$local_refresh_log" ||
   grep -q $'^startup_timing\tlibrary_db_save_failed\t' "$local_log"; then
  emit_thread_sample_artifact_report | tee "$artifact_report" || true
  echo "first scan failed while saving the catalog; latest log follows" >&2
  tail -80 "$local_log" >&2 || true
  exit 1
fi
ready_us="$(sed -n 's/.*"name":"builder_catalog_ready","detail":"elapsed_us=\([0-9][0-9]*\).*/\1/p' "$local_refresh_log" | tail -1)"
saved_us="$(sed -n 's/.*"name":"builder_persisted","detail":"elapsed_us=\([0-9][0-9]*\).*/\1/p' "$local_refresh_log" | tail -1)"
ready_ms="${ready_us:+$(( (ready_us + 500) / 1000 ))}"
saved_ms="${saved_us:+$(( (saved_us + 500) / 1000 ))}"
if [[ -n "$ready_ms" && -n "$saved_ms" ]]; then
  printf 'startup_timing\tlibrary_ready\t%sms\tsource=standalone-builder elapsed_us=%s\n' "$ready_ms" "$ready_us" >"$combined_log"
  printf 'startup_timing\tlibrary_db_saved\t%sms\tsource=standalone-builder elapsed_us=%s\n' "$saved_ms" "$saved_us" >>"$combined_log"
fi
cat "$local_refresh_log" "$local_log" >>"$combined_log"
if [[ -z "$ready_ms" || -z "$saved_ms" ]]; then
  emit_thread_sample_artifact_report | tee "$artifact_report" || true
  echo "first scan did not complete both gates within ${TIMEOUT_SECS}s (library_ready=${ready_ms:-missing}, library_db_saved=${saved_ms:-missing}); latest log follows" >&2
  tail -80 "$combined_log" >&2 || true
  exit 1
fi
gate_failed=0
if ! first_scan_gate_check "$ready_ms" "$saved_ms"; then
  gate_failed=1
  if (( ready_ms > RAM_CATALOG_READY_GATE_MS )); then
    echo "first scan RAM catalog usable gate failed: library_ready=${ready_ms}ms > ${RAM_CATALOG_READY_GATE_MS}ms" >&2
  fi
  if (( saved_ms > DB_SAVE_GATE_MS )); then
    echo "first scan DB save gate failed: library_db_saved=${saved_ms}ms > ${DB_SAVE_GATE_MS}ms" >&2
  fi
fi

awk -v label="$LABEL" -v commit="$commit" -F '\t' '
  BEGIN { OFS = "\t" }
  $1 == "startup_timing" && ($2 == "first_frame" || $2 == "bootstrap_counter_climb" || $2 == "bootstrap_counter_sustained_climb" || $2 == "full_scan_counter_climb" || $2 == "catalog_counter_climb" || $2 == "library_scan_complete" || $2 == "library_db_saved" || $2 == "library_ready" || $2 == "catalog_bridge_sync_update" || $2 == "catalog_worker_ram_catalog") {
    ms = $3
    sub(/ms$/, "", ms)
    if ($2 == "bootstrap_counter_sustained_climb") {
      bootstrap_sustained_ms = ms
      bootstrap_sustained_detail = $4
    }
    if ($2 == "full_scan_counter_climb") {
      full_scan_climb_ms = ms
      full_scan_climb_detail = $4
    }
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
  END {
    if (bootstrap_sustained_ms != "" && full_scan_climb_ms != "") {
      plateau_ms = full_scan_climb_ms - bootstrap_sustained_ms
      print label, commit, "counter_plateau", plateau_ms, "from=" bootstrap_sustained_detail " to=" full_scan_climb_detail
    }
  }
' "$combined_log" >>"$TSV"

db_count="$("$MISTER" db "SELECT count(*) FROM game_rows" 2>/dev/null | awk -F '\t' 'NR > 1 && $1 ~ /^[0-9]+$/ { print $1; exit }' | tr -d '\r' || true)"
status="$("$MISTER" status 2>/dev/null || true)"
printf '%s\t%s\t%s\t%s\t%s\n' "$LABEL" "$commit" "db_count" "0" "$db_count" >>"$TSV"
emit_thread_sample_artifact_report | tee "$artifact_report"

echo "appended to $TSV"
echo "db_count=$db_count"
printf '%s\n' "$status"
if [[ "$gate_failed" -eq 1 ]]; then
  exit 1
fi
