#!/usr/bin/env bash
# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

# Exercise catalog checkpoint drift detection on a real MiSTer.
set -euo pipefail

echo "ERROR: device-catalog-drift-acceptance was retired with Catalog V2; use the V3 rebuild benchmark and catalog acceptance" >&2
exit 2

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
MISTER="${MISTER:-$ROOT/scripts/mister}"
source "$ROOT/scripts/lib/magik-layout.sh"
source "$ROOT/scripts/lib/library-sql-output-lib.sh"
source "$ROOT/scripts/lib/catalog-device-test-lib.sh"
magik_layout_select dev
REMOTE_BIN="$MISTER_MAGIK_BIN"
REMOTE_DB="$MISTER_MAGIK_LIBRARY_DB"
REMOTE_SUMMARY="$MISTER_MAGIK_LIBRARY_SUMMARY"
REMOTE_ENV="$MISTER_MAGIK_LAUNCHER_ENV"
REMOTE_LOG="/tmp/mister-magik-slint.log"
REMOTE_EVENTS="/tmp/mister-magik/events.jsonl"
REMOTE_STATUS="/tmp/mister-magik/status.json"
REMOTE_MARKER="$MISTER_MAGIK_REBUILD_MARKER"
BENCH_DIR="$ROOT/history/toolchain-bench"
TSV="$BENCH_DIR/results-catalog-drift-acceptance.tsv"
LABEL=""
DEPLOY="skip"
REPLACE_LABEL=0
TIMEOUT_SECS=120
KEEP_TEMP=0
WARN_DELAY_SECS=5
ENV_BACKUP=""
HAD_ENV=0
TEMP_KNOWN_CORE=""
TEMP_KNOWN_GAME=""
TEMP_UNKNOWN_CORE=""
TEMP_UNKNOWN_DIR=""
TEMP_NEW_DIR=""

usage() {
  cat <<'EOF'
usage: scripts/device-catalog-drift-acceptance.sh LABEL [--deploy-device|--skip-build] [--replace-label] [--timeout SECS] [--keep-temp] [--no-warn-delay]

Destructively removes the production catalog once, then creates temporary core
and game fixtures to prove checkpoint drift detection for initial creation,
warm unchanged validation, known cores, unknown cores, and new systems.
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
  LABEL="DRIFT-$(date -u +%Y%m%dT%H%M%SZ)"
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
  catalog_device_remote "$1"
}

db() {
  catalog_device_db "$@"
}

last_line() {
  catalog_device_last_line
}

last_number() {
  catalog_device_last_number
}

db_scalar() {
  db "$1" | library_sql_first_result_number
}

sq() {
  catalog_device_shell_quote "$1"
}

sql_string() {
  catalog_device_sql_string "$1"
}

fail() {
  echo "FAIL: $*" >&2
  dump_failure_artifacts >&2 || true
  exit 1
}

dump_failure_artifacts() {
  echo "== slint log tail =="
  remote "tail -180 $(sq "$REMOTE_LOG") 2>/dev/null || true" || true
  echo "== events tail =="
  remote "tail -180 $(sq "$REMOTE_EVENTS") 2>/dev/null || true" || true
  echo "== status =="
  "$MISTER" status --json || true
  echo "== marker =="
  remote "test -e $(sq "$REMOTE_MARKER") && echo marker=present || echo marker=absent" || true
  echo "== checkpoint rows =="
  db "SELECT line FROM catalog_discovery_checkpoint ORDER BY ordinal LIMIT 80;" || true
  echo "== audit rows =="
  db "SELECT catalog_status,core_id,expected_game_dir,reason FROM catalog_audit ORDER BY catalog_status,core_id LIMIT 80;" || true
  echo "== relevant counts =="
  db --query "SELECT 'checkpoint', count(*) FROM catalog_discovery_checkpoint;" \
    --query "SELECT 'launcher_catalog', (SELECT count(*) FROM ui_arcade_preferred) + (SELECT count(*) FROM launcher_catalog_rows);" || true
  "$MISTER" catalog find-launch-ref "$TEMP_KNOWN_GAME" || true
}

wait_remote() {
  catalog_device_wait_remote "$@"
}

assert_remote() {
  catalog_device_assert_remote "$@"
}

assert_db_count() {
  local label="$1"
  local expected="$2"
  local sql="$3"
  local actual
  actual="$(db_scalar "$sql")"
  if [ "$actual" != "$expected" ]; then
    fail "$label expected=$expected actual=${actual:-empty}"
  fi
  echo "ok: $label = $actual"
}

write_launcher_env() {
  local action="${1:-}"
  local env_file
  env_file="$(mktemp)"
  {
    printf 'export MISTER_CATALOG_BACKGROUND_DELAY_MS=0\n'
    printf 'export MISTER_CATALOG_TRACE=detail\n'
    if [ -n "$action" ]; then
      printf 'export MISTER_MAGIK_TEST_LIBRARY_CHANGED_DIALOG_CHOICE=%q\n' "$action"
    fi
  } >"$env_file"
  "$MISTER" put "$env_file" "$REMOTE_ENV" >/dev/null
  rm -f "$env_file"
}

restart_launcher() {
  catalog_device_restart_launcher "${1:-}"
}

force_refresh() {
  catalog_device_force_refresh "$1"
}

restore_launcher_env() {
  catalog_device_restore_launcher_env
}

remove_temp_artifacts() {
  if [ "$KEEP_TEMP" -eq 0 ]; then
    remote "rm -f $(sq "$TEMP_KNOWN_CORE") $(sq "$TEMP_KNOWN_GAME") $(sq "$TEMP_UNKNOWN_CORE"); rm -rf $(sq "$TEMP_UNKNOWN_DIR") $(sq "$TEMP_NEW_DIR"); sync" >/dev/null 2>&1 || true
  fi
}

cleanup() {
  local rc=$?
  set +e
  echo "== cleanup: removing drift fixtures and rebuilding catalog"
  remove_temp_artifacts
  remote "rm -f $(sq "$REMOTE_MARKER")" >/dev/null 2>&1 || true
  restore_launcher_env
  remote "$(sq "$REMOTE_BIN") library-refresh >/tmp/mister-magik-catalog-drift-cleanup.log 2>&1" >/dev/null 2>&1 || {
    echo "cleanup library-refresh failed; log follows" >&2
    remote "tail -180 /tmp/mister-magik-catalog-drift-cleanup.log 2>/dev/null || true" >&2 || true
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

record_bench() {
  local scenario="$1"
  local local_log
  local_log="$(mktemp)"
  "$MISTER" get "$REMOTE_LOG" "$local_log" >/dev/null 2>&1 || true
  local commit changed ready saved checkpoint stamp
  commit="$(git -C "$ROOT" rev-parse --short HEAD 2>/dev/null || echo unknown)"
  changed="$(metric_ms "$local_log" "library_changed_detected")"
  ready="$(metric_ms "$local_log" "library_ready")"
  saved="$(metric_ms "$local_log" "library_db_saved")"
  checkpoint="$(grep -m1 'catalog_checkpoint_tsv.*compute_total' "$local_log" | tr '\t' ' ' || true)"
  stamp="$(metric_ms "$local_log" "catalog_stamp_check")"
  {
    printf '%s\t%s\t%s\tlibrary_changed_detected\t%s\t-\n' "$LABEL" "$commit" "$scenario" "${changed:-}"
    printf '%s\t%s\t%s\tlibrary_ready\t%s\t-\n' "$LABEL" "$commit" "$scenario" "${ready:-}"
    printf '%s\t%s\t%s\tlibrary_db_saved\t%s\t-\n' "$LABEL" "$commit" "$scenario" "${saved:-}"
    printf '%s\t%s\t%s\tcatalog_stamp_check\t%s\t-\n' "$LABEL" "$commit" "$scenario" "${stamp:-}"
    printf '%s\t%s\t%s\tcheckpoint_compute\t-\t%s\n' "$LABEL" "$commit" "$scenario" "${checkpoint:-missing}"
  } >>"$TSV"
  rm -f "$local_log"
}

assert_changed_continue_flow() {
  local scenario="$1"
  restart_launcher "continue"
  wait_remote "$scenario changed detected" "$TIMEOUT_SECS" "grep -q 'library_changed_detected' $(sq "$REMOTE_LOG")"
  wait_remote "$scenario Library changed dialog" "$TIMEOUT_SECS" "grep -q '\"confirm_title\":\"Library changed\"' $(sq "$REMOTE_STATUS")"
  wait_remote "$scenario checkpoint timing" "$TIMEOUT_SECS" "grep -q 'catalog_checkpoint_tsv' $(sq "$REMOTE_LOG")"
  wait_remote "$scenario drift summary" "$TIMEOUT_SECS" "grep -q 'catalog_drift_tsv.*unchanged=false' $(sq "$REMOTE_LOG")"
  wait_remote "$scenario test input chose continue" "$TIMEOUT_SECS" "grep -q 'library_changed_test_dialog_input.*choice=continue button=a' $(sq "$REMOTE_LOG")"
  wait_remote "$scenario marker written" "$TIMEOUT_SECS" "grep -q 'library_rebuild_deferred' $(sq "$REMOTE_LOG") && test -f $(sq "$REMOTE_MARKER")"
  assert_remote "$scenario did not rebuild immediately" "! grep -q 'library_db_saved' $(sq "$REMOTE_LOG")"
  record_bench "$scenario-continue"
}

case "$DEPLOY" in
  device) "$ROOT/scripts/deploy-rust.sh" --device --ui-scope launcher ;;
  skip) : ;;
esac

cat <<EOF
WARNING: catalog drift acceptance test.
This deletes $REMOTE_DB once and creates temporary core/game fixtures.
Cleanup removes fixtures and forces a production library-refresh.
EOF
if [ "$WARN_DELAY_SECS" -gt 0 ]; then
  echo "Starting in ${WARN_DELAY_SECS}s..."
  sleep "$WARN_DELAY_SECS"
fi

ENV_BACKUP="/tmp/mister-magik-launcher.env.${LABEL}.bak"
if [ "$(remote "if [ -f $(sq "$REMOTE_ENV") ]; then cp $(sq "$REMOTE_ENV") $(sq "$ENV_BACKUP"); echo yes; else echo no; fi" | last_line)" = "yes" ]; then
  HAD_ENV=1
fi

TEMP_KNOWN_CORE="/media/fat/_Console/ColecoVision_99999999.rbf"
TEMP_KNOWN_GAME="/media/fat/games/ColecoVision/_mister-magik-${LABEL}.col"
TEMP_UNKNOWN_CORE="/media/fat/_Console/MagiKUnknown${LABEL}.rbf"
TEMP_UNKNOWN_DIR="/media/fat/games/MagiKUnknown${LABEL}"
TEMP_NEW_DIR="/media/fat/games/MagiKNewSystem${LABEL}"

echo "== device catalog drift acceptance label=$LABEL"
remove_temp_artifacts
remote "rm -f $(sq "$REMOTE_MARKER") $(sq "$REMOTE_DB") $(sq "$REMOTE_SUMMARY"); sync"
restart_launcher ""
wait_remote "initial scan visible" "$TIMEOUT_SECS" "grep -q '\"catalog_scan_message\":\"Scanning for games' $(sq "$REMOTE_STATUS")"
wait_remote "initial library ready" "$TIMEOUT_SECS" "grep -q 'library_ready' $(sq "$REMOTE_LOG")"
wait_remote "initial db saved" "$TIMEOUT_SECS" "grep -q 'library_db_saved' $(sq "$REMOTE_LOG")"
assert_db_count "checkpoint table exists" "1" \
  "SELECT count(*) FROM sqlite_master WHERE type='table' AND name='catalog_discovery_checkpoint';"
record_bench "initial-create"

echo "== Warm unchanged checkpoint"
restart_launcher ""
wait_remote "warm stamp check" "$TIMEOUT_SECS" "grep -q 'catalog_stamp_check.*unchanged=true' $(sq "$REMOTE_LOG")"
wait_remote "warm unchanged" "$TIMEOUT_SECS" "grep -q 'library_db_unchanged' $(sq "$REMOTE_LOG")"
assert_remote "warm unchanged did not rebuild" "! grep -q 'library_db_saved' $(sq "$REMOTE_LOG")"
record_bench "warm-unchanged"

echo "== Known core and new game immediate rebuild"
remote "mkdir -p /media/fat/_Console /media/fat/games/ColecoVision; printf 'temporary known core\n' > $(sq "$TEMP_KNOWN_CORE"); printf 'rom\n' > $(sq "$TEMP_KNOWN_GAME"); sync"
restart_launcher "rebuild"
wait_remote "known core drift detected" "$TIMEOUT_SECS" "grep -q 'library_changed_detected' $(sq "$REMOTE_LOG")"
wait_remote "known core rebuild requested" "$TIMEOUT_SECS" "grep -q 'library_rebuild_requested.*source=dialog' $(sq "$REMOTE_LOG")"
wait_remote "known core rebuild saved" "$TIMEOUT_SECS" "grep -q 'library_db_saved' $(sq "$REMOTE_LOG")"
if ! "$MISTER" catalog find-launch-ref "$TEMP_KNOWN_GAME" | awk -F '\t' '
  $4 ~ /^(path|structured|prepared|missing-structured)$/ { found=1 }
  END { exit !found }
'; then
  fail "known temporary game was not cataloged"
fi
echo "ok: known temporary game cataloged"
record_bench "known-core-immediate-rebuild"

echo "== Unknown core with matching game dir continue"
remote "rm -f $(sq "$REMOTE_MARKER"); mkdir -p $(sq "$TEMP_UNKNOWN_DIR"); printf 'temporary unknown core\n' > $(sq "$TEMP_UNKNOWN_CORE"); printf 'payload\n' > $(sq "$TEMP_UNKNOWN_DIR/game.unk"); sync"
assert_changed_continue_flow "unknown-core"

echo "== Marker rebuild after continue"
restart_launcher ""
wait_remote "marker consumed" "$TIMEOUT_SECS" "grep -q 'library_rebuild_marker_consumed' $(sq "$REMOTE_LOG")"
wait_remote "marker rebuild saved" "$TIMEOUT_SECS" "grep -q 'library_db_saved' $(sq "$REMOTE_LOG")"
wait_remote "marker removed" "$TIMEOUT_SECS" "test ! -e $(sq "$REMOTE_MARKER")"
record_bench "marker-rebuild"

echo "== New top-level system dir continue"
remote "rm -f $(sq "$REMOTE_MARKER"); mkdir -p $(sq "$TEMP_NEW_DIR"); printf 'payload\n' > $(sq "$TEMP_NEW_DIR/game.new"); sync"
assert_changed_continue_flow "new-system-dir"
remote "rm -f $(sq "$REMOTE_MARKER")"

echo "device catalog drift acceptance: ok"
