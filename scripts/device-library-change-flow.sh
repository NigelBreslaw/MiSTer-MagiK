#!/usr/bin/env bash
# Exercise the deferred Library changed Continue/Rebuild flow on a real MiSTer.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
MISTER="${MISTER:-$ROOT/scripts/mister}"
REMOTE_BIN="/media/fat/mister-magik/mister-magik-fb"
REMOTE_ENV="/media/fat/mister-magik/launcher.env"
REMOTE_LOG="/tmp/mister-magik-slint.log"
REMOTE_EVENTS="/tmp/mister-magik/events.jsonl"
REMOTE_STATUS="/tmp/mister-magik/status.json"
REMOTE_MARKER="/media/fat/mister-magik/rebuild-on-next-boot"
BENCH_DIR="$ROOT/history/toolchain-bench"
TSV="$BENCH_DIR/results-library-change-flow.tsv"
LABEL=""
DEPLOY="skip"
REPLACE_LABEL=0
TIMEOUT_SECS=90
KEEP_TEMP=0
TEMP_MRA=""
TEMP_TITLE=""
ENV_BACKUP=""
HAD_ENV=0

usage() {
  cat <<'EOF'
usage: scripts/device-library-change-flow.sh LABEL [--deploy-device|--skip-build] [--replace-label] [--timeout SECS] [--keep-temp]

Writes one temporary unique _Arcade .mra, verifies the Library changed Continue
and Rebuild paths, records rebuild timings, then removes the temporary file and
rebuilds the production catalog.
EOF
}

while [ "$#" -gt 0 ]; do
  case "$1" in
    --deploy-device) DEPLOY="device"; shift ;;
    --skip-build) DEPLOY="skip"; shift ;;
    --replace-label) REPLACE_LABEL=1; shift ;;
    --timeout) TIMEOUT_SECS="${2:?--timeout needs seconds}"; shift 2 ;;
    --keep-temp) KEEP_TEMP=1; shift ;;
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
  LABEL="LIBCHANGE-$(date -u +%Y%m%dT%H%M%SZ)"
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
  printf 'label\tcommit\tmode\tevent\tms\tnotes\n' >"$TSV"
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
  remote "tail -120 $(sq "$REMOTE_LOG") 2>/dev/null || true" || true
  echo "== events tail =="
  remote "tail -120 $(sq "$REMOTE_EVENTS") 2>/dev/null || true" || true
  echo "== status =="
  "$MISTER" status --json || true
  echo "== process list =="
  remote "ps w | grep -E 'MiSTer|MiSTer_MagiK|mister-magik-fb' | grep -v grep || true" || true
  echo "== marker =="
  remote "test -e $(sq "$REMOTE_MARKER") && echo marker=present || echo marker=absent" || true
  echo "== db counts =="
  db "SELECT 'games', count(*) FROM games;" || true
  db "SELECT 'launcher_catalog', count(*) FROM launcher_catalog;" || true
  if [ -n "$TEMP_MRA" ]; then
    db "SELECT 'temp_mra_payload', count(*) FROM payloads_text WHERE file_path=$(sql_string "$TEMP_MRA");" || true
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

launcher_count() {
  remote "ps w | grep '[m]ister-magik-fb ui launcher' | wc -l" | last_number
}

write_launcher_env() {
  local action="${1:-}"
  local env_file
  env_file="$(mktemp)"
  {
    printf 'export MISTER_CATALOG_BACKGROUND_DELAY_MS=0\n'
    if [ -n "$action" ]; then
      printf 'export MISTER_MAGIK_TEST_LIBRARY_CHANGED_DIALOG_CHOICE=%q\n' "$action"
    fi
  } >"$env_file"
  "$MISTER" put "$env_file" "$REMOTE_ENV" >/dev/null
  rm -f "$env_file"
}

restart_launcher() {
  local action="${1:-}"
  write_launcher_env "$action"
  remote "rm -f $(sq "$REMOTE_LOG") $(sq "$REMOTE_EVENTS"); if [ ! -p /dev/MiSTer_cmd ]; then echo 'missing /dev/MiSTer_cmd'; exit 12; fi; printf 'mister_magik_restart_launcher\n' > /dev/MiSTer_cmd"
  wait_remote "launcher process" 25 "test \"\$(ps w | grep '[m]ister-magik-fb ui launcher' | wc -l)\" = 1"
}

force_refresh() {
  local log="$1"
  remote "$(sq "$REMOTE_BIN") library-refresh >$(sq "$log") 2>&1"
}

temp_mra_count() {
  db "SELECT count(*) FROM payloads_text WHERE file_path=$(sql_string "$TEMP_MRA");" | last_number
}

assert_temp_mra_count() {
  local expected="$1"
  assert_db_count "temp MRA payload row count" "$expected" \
    "SELECT count(*) FROM payloads_text WHERE file_path=$(sql_string "$TEMP_MRA");"
}

write_temp_mra() {
  remote "printf '%s\n' '<mra>' '<name>$TEMP_TITLE</name>' '</mra>' > $(sq "$TEMP_MRA"); sync"
  assert_remote "temporary MRA exists" "test -f $(sq "$TEMP_MRA")"
}

assert_temp_new_discovery_projection() {
  assert_db_count "temp MRA game discovery timestamp" "1" \
    "SELECT count(*) FROM games JOIN launch_plans ON launch_plans.game_id=games.game_id WHERE launch_plans.launch_ref=$(sql_string "$TEMP_MRA") AND games.discovered_at_unix IS NOT NULL;"
  assert_db_count "temp MRA launcher catalog discovery timestamp" "1" \
    "SELECT count(*) FROM launcher_catalog WHERE launch_ref=$(sql_string "$TEMP_MRA") AND discovered_at_unix IS NOT NULL;"
  assert_db_count "temp MRA arcade list discovery timestamp" "1" \
    "SELECT count(*) FROM ui_arcade_preferred WHERE launch_ref=$(sql_string "$TEMP_MRA") AND discovered_at_unix IS NOT NULL;"
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
  echo "== cleanup: removing test artifacts and rebuilding catalog"
  remove_temp_mra
  remote "rm -f $(sq "$REMOTE_MARKER")" >/dev/null 2>&1 || true
  restore_launcher_env
  remote "$(sq "$REMOTE_BIN") library-refresh >/tmp/mister-magik-library-change-cleanup.log 2>&1" >/dev/null 2>&1 || {
    echo "cleanup library-refresh failed; log follows" >&2
    remote "tail -120 /tmp/mister-magik-library-change-cleanup.log 2>/dev/null || true" >&2 || true
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

metric_ms_after() {
  local log="$1"
  local event="$2"
  local after_event="$3"
  awk -F '\t' -v event="$event" -v after_event="$after_event" '
    $1 == "startup_timing" && $2 == after_event && after_ms == "" {
      after_ms=$3
      sub(/ms$/, "", after_ms)
    }
    $1 == "startup_timing" && $2 == event && after_ms != "" {
      ms=$3
      sub(/ms$/, "", ms)
      if (ms >= after_ms) {
        print ms
        exit
      }
    }
  ' "$log"
}

record_bench() {
  local mode="$1"
  local start_event="$2"
  local local_log
  local_log="$(mktemp)"
  "$MISTER" get "$REMOTE_LOG" "$local_log" >/dev/null 2>&1 || true
  local commit
  commit="$(git -C "$ROOT" rev-parse --short HEAD 2>/dev/null || echo unknown)"
  local changed start scan saved ready duration
  start="$(metric_ms "$local_log" "$start_event")"
  changed="$(metric_ms "$local_log" "library_changed_detected")"
  scan="$(metric_ms_after "$local_log" "library_scan_complete" "$start_event")"
  saved="$(metric_ms_after "$local_log" "library_db_saved" "$start_event")"
  ready="$(metric_ms_after "$local_log" "library_ready" "$start_event")"
  if [ -n "$start" ] && [ -n "$saved" ]; then
    duration=$((saved - start))
  else
    duration=""
  fi
  {
    printf '%s\t%s\t%s\tlibrary_changed_detected\t%s\t-\n' "$LABEL" "$commit" "$mode" "${changed:-}"
    printf '%s\t%s\t%s\t%s\t%s\t-\n' "$LABEL" "$commit" "$mode" "$start_event" "${start:-}"
    printf '%s\t%s\t%s\tlibrary_scan_complete\t%s\t-\n' "$LABEL" "$commit" "$mode" "${scan:-}"
    printf '%s\t%s\t%s\tlibrary_db_saved\t%s\t-\n' "$LABEL" "$commit" "$mode" "${saved:-}"
    printf '%s\t%s\t%s\tlibrary_ready\t%s\t-\n' "$LABEL" "$commit" "$mode" "${ready:-}"
    printf '%s\t%s\t%s\trebuild_save_duration\t%s\tthreshold_ms=60000\n' "$LABEL" "$commit" "$mode" "${duration:-}"
  } >>"$TSV"
  rm -f "$local_log"
  if [ -z "$duration" ] || [ "$duration" -gt 60000 ]; then
    fail "$mode rebuild save duration invalid_or_slow=${duration:-empty}"
  fi
  echo "ok: $mode rebuild save duration ${duration}ms"
}

case "$DEPLOY" in
  device) "$ROOT/scripts/deploy-rust.sh" --device --ui-scope launcher ;;
  skip) : ;;
esac

ENV_BACKUP="/tmp/mister-magik-launcher.env.${LABEL}.bak"
if [ "$(remote "if [ -f $(sq "$REMOTE_ENV") ]; then cp $(sq "$REMOTE_ENV") $(sq "$ENV_BACKUP"); echo yes; else echo no; fi" | last_line)" = "yes" ]; then
  HAD_ENV=1
fi

TEMP_MRA="/media/fat/_Arcade/_mister-magik-it-${LABEL}.mra"
TEMP_TITLE="MiSTer MagiK IT ${LABEL}"

echo "== device library-change flow label=$LABEL temp=$TEMP_MRA"
assert_db_count "launcher_catalog table exists" "1" "SELECT count(*) FROM sqlite_master WHERE type='table' AND name='launcher_catalog';"
assert_remote "single launcher process before test" "test \"\$(ps w | grep '[m]ister-magik-fb ui launcher' | wc -l)\" = 1"

echo "== Continue path"
remote "rm -f $(sq "$REMOTE_MARKER") $(sq "$TEMP_MRA")"
write_temp_mra
assert_temp_mra_count 0
restart_launcher "continue"
wait_remote "library changed detected" "$TIMEOUT_SECS" "grep -q 'library_changed_detected' $(sq "$REMOTE_LOG")"
wait_remote "library changed dialog status" "$TIMEOUT_SECS" "grep -q '\"confirm_title\":\"Library changed\"' $(sq "$REMOTE_STATUS")"
wait_remote "test input chose continue" "$TIMEOUT_SECS" "grep -q 'library_changed_test_dialog_input.*choice=continue button=a' $(sq "$REMOTE_LOG")"
wait_remote "rebuild deferred marker written" "$TIMEOUT_SECS" "grep -q 'library_rebuild_deferred' $(sq "$REMOTE_LOG") && test -f $(sq "$REMOTE_MARKER")"
assert_remote "continue did not save database in same session" "! grep -q 'library_db_saved' $(sq "$REMOTE_LOG")"
assert_temp_mra_count 0

echo "== Deferred marker rebuild path"
restart_launcher ""
wait_remote "marker consumed" "$TIMEOUT_SECS" "grep -q 'library_rebuild_marker_consumed' $(sq "$REMOTE_LOG")"
wait_remote "updating library status" "$TIMEOUT_SECS" "grep -q '\"catalog_scan_message\":\"Updating Library\"' $(sq "$REMOTE_STATUS")"
wait_remote "marker removed" "$TIMEOUT_SECS" "test ! -e $(sq "$REMOTE_MARKER")"
wait_remote "deferred rebuild saved database" "$TIMEOUT_SECS" "grep -q 'library_db_saved' $(sq "$REMOTE_LOG")"
assert_temp_mra_count 1
assert_temp_new_discovery_projection
record_bench "deferred-marker" "library_rebuild_marker_consumed"

echo "== Immediate Rebuild path"
remove_temp_mra
force_refresh "/tmp/mister-magik-library-change-baseline.log"
assert_temp_mra_count 0
write_temp_mra
assert_temp_mra_count 0
restart_launcher "rebuild"
wait_remote "library changed detected for rebuild" "$TIMEOUT_SECS" "grep -q 'library_changed_detected' $(sq "$REMOTE_LOG")"
wait_remote "library changed dialog status for rebuild" "$TIMEOUT_SECS" "grep -q '\"confirm_title\":\"Library changed\"' $(sq "$REMOTE_STATUS")"
wait_remote "test input selected rebuild" "$TIMEOUT_SECS" "grep -q 'library_changed_test_dialog_input.*choice=rebuild button=right' $(sq "$REMOTE_LOG")"
wait_remote "test input confirmed rebuild" "$TIMEOUT_SECS" "grep -q 'library_changed_test_dialog_input.*choice=rebuild button=a' $(sq "$REMOTE_LOG")"
wait_remote "dialog rebuild requested" "$TIMEOUT_SECS" "grep -q 'library_rebuild_requested.*source=dialog' $(sq "$REMOTE_LOG")"
wait_remote "immediate rebuild updating status" "$TIMEOUT_SECS" "grep -q '\"catalog_scan_message\":\"Updating Library\"' $(sq "$REMOTE_STATUS")"
wait_remote "immediate rebuild saved database" "$TIMEOUT_SECS" "grep -q 'library_db_saved' $(sq "$REMOTE_LOG")"
assert_remote "immediate rebuild wrote no marker" "test ! -e $(sq "$REMOTE_MARKER")"
assert_temp_mra_count 1
assert_temp_new_discovery_projection
record_bench "immediate-rebuild" "library_rebuild_requested"

echo "device library-change flow: ok"
