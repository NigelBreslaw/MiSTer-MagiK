#!/usr/bin/env bash
# Break production catalog startup state on a real MiSTer and prove recovery.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
MISTER="${MISTER:-$ROOT/scripts/mister}"
REMOTE_BIN="/media/fat/mister-magik/mister-magik-fb"
REMOTE_DB="/media/fat/mister-magik/library.sqlite3"
REMOTE_ENV="/media/fat/mister-magik/launcher.env"
REMOTE_LOG="/tmp/mister-magik-slint.log"
REMOTE_EVENTS="/tmp/mister-magik/events.jsonl"
REMOTE_STATUS="/tmp/mister-magik/status.json"
REMOTE_MARKER="/media/fat/mister-magik/rebuild-on-next-boot"
BENCH_DIR="$ROOT/history/toolchain-bench"
TSV="$BENCH_DIR/results-catalog-destruction.tsv"
LABEL=""
DEPLOY="skip"
REPLACE_LABEL=0
TIMEOUT_SECS=120
KEEP_TEMP=0
WARN_DELAY_SECS=5
TEMP_MRA=""
SOURCE_MRA=""
ENV_BACKUP=""
HAD_ENV=0

usage() {
  cat <<'EOF'
usage: scripts/device-catalog-destruction.sh LABEL [--deploy-device|--skip-build] [--replace-label] [--timeout SECS] [--keep-temp] [--no-warn-delay]

Destructively mutates /media/fat/mister-magik/library.sqlite3 on a real MiSTer:
missing DB, zero-byte DB, corrupt DB, bad marker plus bad DB, and a real
_Arcade file-change detection path. The script does not back up the production
DB; it verifies recovery and finishes with a forced library-refresh cleanup.
EOF
}

while [ "$#" -gt 0 ]; do
  case "$1" in
    --deploy-device) DEPLOY="device"; shift ;;
    --skip-build) DEPLOY="skip"; shift ;;
    --replace-label) REPLACE_LABEL=1; shift ;;
    --timeout) TIMEOUT_SECS="${2:?--timeout needs seconds}"; shift 2 ;;
    --keep-temp) KEEP_TEMP=1; shift ;;
    --no-warn-delay) WARN_DELAY_SECS=0; shift ;;
    -h|--help) usage; exit 0 ;;
    --*) echo "unknown option: $1" >&2; usage >&2; exit 2 ;;
    *)
      if [ -n "$LABEL" ]; then
        echo "unexpected argument: $1" >&2
        usage >&2
        exit 2
      fi
      LABEL="$1"
      shift
      ;;
  esac
done

if [ -z "$LABEL" ]; then
  LABEL="DESTROY-$(date -u +%Y%m%dT%H%M%SZ)"
fi
if [[ ! "$LABEL" =~ ^[A-Za-z0-9_.-]+$ ]]; then
  echo "label must contain only letters, numbers, _, ., or -" >&2
  exit 2
fi
if [[ ! "$TIMEOUT_SECS" =~ ^[0-9]+$ ]]; then
  echo "--timeout must be an integer" >&2
  exit 2
fi

mkdir -p "$BENCH_DIR"
if [ ! -f "$TSV" ]; then
  printf 'label\tcommit\tscenario\tevent\tms\tnotes\n' >"$TSV"
elif [ "$REPLACE_LABEL" -eq 1 ]; then
  tmp="$(mktemp)"
  { head -1 "$TSV"; grep -v "^${LABEL}	" "$TSV" | tail -n +2; } >"$tmp" || true
  mv "$tmp" "$TSV"
fi

remote() {
  "$MISTER" run "$1"
}

db() {
  "$MISTER" db "$1"
}

last_line() {
  awk 'NF { value=$0 } END { print value }' | tr -d '\r'
}

last_number() {
  awk 'NF { value=$NF } END { gsub(/[^0-9]/, "", value); print value }'
}

sq() {
  printf "'%s'" "$(printf "%s" "$1" | sed "s/'/'\\\\''/g")"
}

sql_string() {
  printf "'%s'" "$(printf "%s" "$1" | sed "s/'/''/g")"
}

fail() {
  echo "FAIL: $*" >&2
  dump_failure_artifacts >&2 || true
  exit 1
}

dump_failure_artifacts() {
  echo "== slint log tail =="
  remote "tail -160 $(sq "$REMOTE_LOG") 2>/dev/null || true" || true
  echo "== events tail =="
  remote "tail -160 $(sq "$REMOTE_EVENTS") 2>/dev/null || true" || true
  echo "== status =="
  "$MISTER" status --json || true
  echo "== process list =="
  remote "ps w | grep -E 'MiSTer|MiSTer_MagiK|mister-magik-fb' | grep -v grep || true" || true
  echo "== marker =="
  remote "test -e $(sq "$REMOTE_MARKER") && echo marker=present || echo marker=absent" || true
  echo "== db file =="
  remote "ls -l $(sq "$REMOTE_DB") 2>/dev/null || true" || true
  echo "== db counts =="
  db "SELECT 'games', count(*) FROM games;" || true
  db "SELECT 'launcher_catalog', count(*) FROM launcher_catalog;" || true
  if [ -n "$TEMP_MRA" ]; then
    db "SELECT 'temp_mra_payload', count(*) FROM payloads WHERE file_path=$(sql_string "$TEMP_MRA");" || true
  fi
}

wait_remote() {
  local label="$1"
  local timeout="$2"
  local command="$3"
  local deadline=$((SECONDS + timeout))
  while [ "$SECONDS" -lt "$deadline" ]; do
    if remote "$command" >/dev/null 2>&1; then
      echo "ok: $label"
      return 0
    fi
    sleep 1
  done
  fail "timeout waiting for $label"
}

assert_remote() {
  local label="$1"
  local command="$2"
  if ! remote "$command" >/dev/null 2>&1; then
    fail "$label"
  fi
  echo "ok: $label"
}

assert_db_count() {
  local label="$1"
  local expected="$2"
  local sql="$3"
  local actual
  actual="$(db "$sql" | last_number)"
  if [ "$actual" != "$expected" ]; then
    fail "$label expected=$expected actual=${actual:-empty}"
  fi
  echo "ok: $label = $actual"
}

assert_single_launcher() {
  assert_remote "single launcher process" "test \"\$(ps w | grep '[m]ister-magik-fb ui launcher' | wc -l)\" = 1"
}

assert_no_refresh_process() {
  assert_remote "no active library-refresh process" "test \"\$(ps w | grep '[m]ister-magik-fb library-refresh' | wc -l)\" = 0"
}

write_launcher_env() {
  local action="${1:-}"
  local env_file
  env_file="$(mktemp)"
  {
    printf 'export MISTER_CATALOG_BACKGROUND_DELAY_MS=0\n'
    if [ -n "$action" ]; then
      printf 'export MISTER_MAGIK_TEST_LIBRARY_CHANGED_ACTION=%q\n' "$action"
    fi
  } >"$env_file"
  "$MISTER" put "$env_file" "$REMOTE_ENV" >/dev/null
  rm -f "$env_file"
}

restart_launcher() {
  local action="${1:-}"
  write_launcher_env "$action"
  remote "rm -f $(sq "$REMOTE_LOG") $(sq "$REMOTE_EVENTS") $(sq "$REMOTE_STATUS"); if [ ! -p /dev/MiSTer_cmd ]; then echo 'missing /dev/MiSTer_cmd'; exit 12; fi; printf 'mister_magik_restart_launcher\n' > /dev/MiSTer_cmd"
  wait_remote "launcher process" 25 "test \"\$(ps w | grep '[m]ister-magik-fb ui launcher' | wc -l)\" = 1"
}

force_refresh() {
  local log="$1"
  remote "$(sq "$REMOTE_BIN") library-refresh >$(sq "$log") 2>&1"
}

temp_mra_count() {
  db "SELECT count(*) FROM payloads WHERE file_path=$(sql_string "$TEMP_MRA");" | last_number
}

assert_temp_mra_count() {
  local expected="$1"
  assert_db_count "temp MRA payload row count" "$expected" \
    "SELECT count(*) FROM payloads WHERE file_path=$(sql_string "$TEMP_MRA");"
}

copy_temp_mra() {
  remote "cp $(sq "$SOURCE_MRA") $(sq "$TEMP_MRA"); sync"
  assert_remote "temporary MRA exists" "test -f $(sq "$TEMP_MRA")"
}

remove_temp_mra() {
  if [ -n "$TEMP_MRA" ] && [ "$KEEP_TEMP" -eq 0 ]; then
    remote "rm -f $(sq "$TEMP_MRA"); sync" >/dev/null 2>&1 || true
  fi
}

restore_launcher_env() {
  if [ "$HAD_ENV" -eq 1 ] && [ -n "$ENV_BACKUP" ]; then
    remote "mv $(sq "$ENV_BACKUP") $(sq "$REMOTE_ENV")" >/dev/null 2>&1 || true
  else
    remote "rm -f $(sq "$REMOTE_ENV")" >/dev/null 2>&1 || true
  fi
}

cleanup() {
  local rc=$?
  set +e
  echo "== cleanup: removing destructive artifacts and rebuilding catalog"
  remove_temp_mra
  remote "rm -f $(sq "$REMOTE_MARKER")" >/dev/null 2>&1 || true
  restore_launcher_env
  remote "$(sq "$REMOTE_BIN") library-refresh >/tmp/mister-magik-catalog-destruction-cleanup.log 2>&1" >/dev/null 2>&1 || {
    echo "cleanup library-refresh failed; log follows" >&2
    remote "tail -160 /tmp/mister-magik-catalog-destruction-cleanup.log 2>/dev/null || true" >&2 || true
  }
  remote "if [ -p /dev/MiSTer_cmd ]; then printf 'mister_magik_restart_launcher\n' > /dev/MiSTer_cmd; fi" >/dev/null 2>&1 || true
  exit "$rc"
}
trap cleanup EXIT

metric_ms() {
  local log="$1"
  local event="$2"
  awk -F '\t' -v event="$event" '$1 == "startup_timing" && $2 == event { ms=$3; sub(/ms$/, "", ms); print ms; exit }' "$log"
}

record_rebuild_bench() {
  local scenario="$1"
  local start_event="$2"
  local local_log
  local_log="$(mktemp)"
  "$MISTER" get "$REMOTE_LOG" "$local_log" >/dev/null 2>&1 || true
  local commit
  commit="$(git -C "$ROOT" rev-parse --short HEAD 2>/dev/null || echo unknown)"
  local start failed empty saved ready duration
  start="$(metric_ms "$local_log" "$start_event")"
  failed="$(metric_ms "$local_log" "catalog_cache_load_failed")"
  empty="$(metric_ms "$local_log" "catalog_cache_empty")"
  saved="$(metric_ms "$local_log" "library_db_saved")"
  ready="$(metric_ms "$local_log" "library_ready")"
  if [ -n "$start" ] && [ -n "$ready" ]; then
    duration=$((ready - start))
  else
    duration="$ready"
  fi
  {
    printf '%s\t%s\t%s\tcatalog_cache_load_failed\t%s\t-\n' "$LABEL" "$commit" "$scenario" "${failed:-}"
    printf '%s\t%s\t%s\tcatalog_cache_empty\t%s\t-\n' "$LABEL" "$commit" "$scenario" "${empty:-}"
    if [ "$start_event" != "catalog_cache_load_failed" ] && [ "$start_event" != "catalog_cache_empty" ]; then
      printf '%s\t%s\t%s\t%s\t%s\t-\n' "$LABEL" "$commit" "$scenario" "$start_event" "${start:-}"
    fi
    printf '%s\t%s\t%s\tlibrary_db_saved\t%s\t-\n' "$LABEL" "$commit" "$scenario" "${saved:-}"
    printf '%s\t%s\t%s\tlibrary_ready\t%s\t-\n' "$LABEL" "$commit" "$scenario" "${ready:-}"
    printf '%s\t%s\t%s\trebuild_duration\t%s\tthreshold_ms=60000\n' "$LABEL" "$commit" "$scenario" "${duration:-}"
  } >>"$TSV"
  rm -f "$local_log"
  if [ -z "$duration" ] || [ "$duration" -gt 60000 ]; then
    fail "$scenario rebuild duration invalid_or_slow=${duration:-empty}"
  fi
  echo "ok: $scenario rebuild duration ${duration}ms"
}

record_file_change_bench() {
  local scenario="$1"
  local local_log
  local_log="$(mktemp)"
  "$MISTER" get "$REMOTE_LOG" "$local_log" >/dev/null 2>&1 || true
  local commit changed action
  commit="$(git -C "$ROOT" rev-parse --short HEAD 2>/dev/null || echo unknown)"
  changed="$(metric_ms "$local_log" "library_changed_detected")"
  action="$(metric_ms "$local_log" "library_changed_test_action")"
  {
    printf '%s\t%s\t%s\tlibrary_changed_detected\t%s\t-\n' "$LABEL" "$commit" "$scenario" "${changed:-}"
    printf '%s\t%s\t%s\tlibrary_changed_test_action\t%s\t-\n' "$LABEL" "$commit" "$scenario" "${action:-}"
  } >>"$TSV"
  rm -f "$local_log"
}

require_first_run_scan() {
  local scenario="$1"
  wait_remote "$scenario first-run scan copy" "$TIMEOUT_SECS" \
    "grep -q '\"catalog_scan_message\":\"Scanning for games' $(sq "$REMOTE_STATUS") && grep -q '\"catalog_scan_visible\":true' $(sq "$REMOTE_STATUS")"
  assert_remote "$scenario did not show Library changed dialog" \
    "! grep -q '\"confirm_title\":\"Library changed\"' $(sq "$REMOTE_STATUS")"
}

require_ready_rebuild() {
  local scenario="$1"
  local start_event="$2"
  wait_remote "$scenario library ready" "$TIMEOUT_SECS" "grep -q 'library_ready' $(sq "$REMOTE_LOG")"
  wait_remote "$scenario DB present" 15 "test -s $(sq "$REMOTE_DB")"
  assert_single_launcher
  assert_no_refresh_process
  assert_remote "$scenario marker absent" "test ! -e $(sq "$REMOTE_MARKER")"
  record_rebuild_bench "$scenario" "$start_event"
}

case "$DEPLOY" in
  device) "$ROOT/scripts/deploy-rust.sh" --device --ui-scope launcher ;;
  skip) : ;;
esac

cat <<EOF
WARNING: destructive catalog startup test.
This will delete, truncate, and corrupt $REMOTE_DB on the MiSTer.
No DB backup is made. Cleanup will force a fresh library-refresh.
EOF
if [ "$WARN_DELAY_SECS" -gt 0 ]; then
  echo "Starting in ${WARN_DELAY_SECS}s..."
  sleep "$WARN_DELAY_SECS"
fi

ENV_BACKUP="/tmp/mister-magik-launcher.env.${LABEL}.bak"
if [ "$(remote "if [ -f $(sq "$REMOTE_ENV") ]; then cp $(sq "$REMOTE_ENV") $(sq "$ENV_BACKUP"); echo yes; else echo no; fi" | last_line)" = "yes" ]; then
  HAD_ENV=1
fi

SOURCE_MRA="$(db "SELECT launch_ref FROM launcher_catalog WHERE launch_ref LIKE '/media/fat/_Arcade/%.mra' AND launch_ref NOT LIKE '%_mister-magik-it-%' ORDER BY launch_ref LIMIT 1;" | last_line)"
if [ -z "$SOURCE_MRA" ] || [[ "$SOURCE_MRA" != /media/fat/_Arcade/*.mra ]]; then
  fail "could not find source _Arcade MRA in launcher_catalog"
fi
TEMP_MRA="/media/fat/_Arcade/_mister-magik-it-${LABEL}.mra"

echo "== device catalog destruction label=$LABEL source=$SOURCE_MRA temp=$TEMP_MRA"
assert_db_count "launcher_catalog table exists" "1" "SELECT count(*) FROM sqlite_master WHERE type='table' AND name='launcher_catalog';"
assert_single_launcher

echo "== Missing DB recovery"
remote "rm -f $(sq "$REMOTE_MARKER") $(sq "$TEMP_MRA") $(sq "$REMOTE_DB"); sync"
restart_launcher ""
wait_remote "missing-db cache load failed" "$TIMEOUT_SECS" "grep -q 'catalog_cache_load_failed' $(sq "$REMOTE_LOG")"
require_first_run_scan "missing-db"
require_ready_rebuild "missing-db" "catalog_cache_load_failed"

echo "== Zero-byte DB recovery"
remote "rm -f $(sq "$REMOTE_MARKER") $(sq "$TEMP_MRA"); : > $(sq "$REMOTE_DB"); sync"
restart_launcher ""
wait_remote "zero-byte DB detected" "$TIMEOUT_SECS" "grep -Eq 'catalog_cache_load_failed|catalog_cache_empty' $(sq "$REMOTE_LOG")"
require_first_run_scan "zero-byte-db"
require_ready_rebuild "zero-byte-db" "catalog_cache_load_failed"

echo "== Corrupt DB recovery"
remote "rm -f $(sq "$REMOTE_MARKER") $(sq "$TEMP_MRA"); printf 'not-a-sqlite-db-for-magik\n' > $(sq "$REMOTE_DB"); sync"
restart_launcher ""
wait_remote "corrupt DB detected" "$TIMEOUT_SECS" "grep -q 'catalog_cache_load_failed' $(sq "$REMOTE_LOG")"
require_first_run_scan "corrupt-db"
require_ready_rebuild "corrupt-db" "catalog_cache_load_failed"

echo "== Bad marker plus bad DB recovery"
remote "printf 'stale marker from destruction test\n' > $(sq "$REMOTE_MARKER"); printf 'bad sqlite with marker\n' > $(sq "$REMOTE_DB"); sync"
restart_launcher ""
wait_remote "bad marker consumed" "$TIMEOUT_SECS" "grep -q 'library_rebuild_marker_consumed' $(sq "$REMOTE_LOG")"
wait_remote "marker bad DB shows updating library" "$TIMEOUT_SECS" "grep -q '\"catalog_scan_message\":\"Updating Library\"' $(sq "$REMOTE_STATUS")"
require_ready_rebuild "bad-marker-bad-db" "library_rebuild_marker_consumed"

echo "== Real file-change detection still uses Library changed"
remove_temp_mra
force_refresh "/tmp/mister-magik-catalog-destruction-baseline.log"
assert_temp_mra_count 0
copy_temp_mra
assert_temp_mra_count 0
restart_launcher "continue"
wait_remote "file-change detected" "$TIMEOUT_SECS" "grep -q 'library_changed_detected' $(sq "$REMOTE_LOG")"
wait_remote "file-change Library changed dialog" "$TIMEOUT_SECS" "grep -q '\"confirm_title\":\"Library changed\"' $(sq "$REMOTE_STATUS")"
wait_remote "file-change test hook chose continue" "$TIMEOUT_SECS" "grep -q 'library_changed_test_action.*action=continue' $(sq "$REMOTE_LOG")"
wait_remote "file-change marker written" "$TIMEOUT_SECS" "grep -q 'library_rebuild_deferred' $(sq "$REMOTE_LOG") && test -f $(sq "$REMOTE_MARKER")"
assert_remote "file-change did not rebuild in same session" "! grep -q 'library_db_saved' $(sq "$REMOTE_LOG")"
assert_single_launcher
record_file_change_bench "file-change-continue"
remote "rm -f $(sq "$REMOTE_MARKER")"

echo "device catalog destruction: ok"
