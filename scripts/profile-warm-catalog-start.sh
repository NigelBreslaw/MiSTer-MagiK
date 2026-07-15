#!/usr/bin/env bash
# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

# Measure warm launcher startup catalog timing without forcing a rebuild.
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
MISTER="$HERE/scripts/mister"
REMOTE_ENV="/media/fat/mister-magik/launcher.env"
REMOTE_LOG="/tmp/mister-magik-slint.log"
BENCH_DIR="$HERE/history/toolchain-bench"
TSV="$BENCH_DIR/results-warm-catalog.tsv"
TSV_HEADER="label	iteration	first_frame_ms	first_frame_catalog_ready	catalog_cache_load_sync_ms	catalog_cache_load_sync_total_us	catalog_summary_load_ms	catalog_summary_load_us	catalog_bridge_systems_us	catalog_bridge_sync_us	full_catalog_ready_ms	full_catalog_ready_load_us	catalog_load_open_us	catalog_load_schema_check_us	catalog_load_query_us	catalog_load_query_prepare_us	catalog_load_query_first_row_us	catalog_load_query_row_read_us	catalog_load_query_row_hydrate_us	catalog_load_launch_plans_us	catalog_load_systems_us	catalog_load_catalog_us	catalog_stamp_check_ms	catalog_stamp_unchanged	catalog_stamp_check_us	catalog_stamp_compute_us	catalog_stamp_open_us	catalog_stamp_read_us	catalog_stamp_checkpoint_read_us	catalog_stamp_compare_us	catalog_stamp_checkpoint_compare_us	library_db_unchanged_ms	result"
WARM_VALIDATION_GATE_US=2000000
VALIDATION_TIMEOUT_SECS=30

LABEL=""
ITERATIONS=1
REPLACE_LABEL=0
DEPLOY=0

usage() {
  cat <<'EOF'
Usage: scripts/profile-warm-catalog-start.sh LABEL [--replace-label] [--iterations N] [--deploy-device]
       scripts/profile-warm-catalog-start.sh --self-test

Restarts the production launcher with the default catalog refresh policy and
records warm catalog startup timings from startup_timing log rows. It does not
force a rebuild and does not launch a core.
EOF
}

warm_catalog_parse_log() {
  local label="$1" iteration="$2" log="$3"
  awk -F '\t' -v label="$label" -v iteration="$iteration" -v validation_gate_us="$WARM_VALIDATION_GATE_US" '
    BEGIN { OFS = "\t" }
    function ms_for(event,   i, ms) {
      for (i = n; i >= 1; i--) if (name[i] == event) {
        ms = at_ms[i]; sub(/ms$/, "", ms); return ms + 0
      }
      return -1
    }
    function detail_for(event,   i) {
      for (i = n; i >= 1; i--) if (name[i] == event) return detail[i]
      return ""
    }
    function field(text, key,   parts, count, i, prefix) {
      count = split(text, parts, " ")
      prefix = key "="
      for (i = 1; i <= count; i++) {
        if (index(parts[i], prefix) == 1) return substr(parts[i], length(prefix) + 1)
      }
      return ""
    }
    $1 == "startup_timing" {
      n++
      name[n] = $2
      at_ms[n] = $3
      detail[n] = $4
    }
    END {
      first = ms_for("first_frame")
      first_detail = detail_for("first_frame")
      first_ready = field(first_detail, "catalog_ready")
      sync_ms = ms_for("catalog_cache_load_sync")
      sync_total = field(detail_for("catalog_cache_load_sync"), "total_us")
      summary_ms = ms_for("catalog_summary_load")
      summary_us = field(detail_for("catalog_summary_load"), "elapsed_us")
      bridge_systems = field(detail_for("catalog_bridge_systems"), "elapsed_us")
      bridge_sync = field(detail_for("catalog_bridge_sync"), "elapsed_us")
      ready_ms = ms_for("library_ready")
      ready_load_us = field(detail_for("library_ready"), "load_us")
      load_detail = detail_for("catalog_worker_cache_load")
      if (load_detail == "") load_detail = detail_for("catalog_worker_navigation_load")
      load_open = field(load_detail, "open_us")
      load_schema = field(load_detail, "schema_check_us")
      load_query = field(load_detail, "query_us")
      load_prepare = field(load_detail, "query_prepare_us")
      load_first_row = field(load_detail, "query_first_row_us")
      load_row_read = field(load_detail, "query_row_read_us")
      load_row_hydrate = field(load_detail, "query_row_hydrate_us")
      load_launch_plans = field(load_detail, "launch_plans_us")
      load_systems = field(load_detail, "systems_us")
      load_catalog = field(load_detail, "catalog_us")
      stamp_ms = ms_for("catalog_stamp_check")
      stamp_detail = detail_for("catalog_stamp_check")
      stamp_unchanged = field(stamp_detail, "unchanged")
      stamp_check_us = field(stamp_detail, "check_us")
      stamp_compute_us = field(stamp_detail, "compute_us")
      stamp_open_us = field(stamp_detail, "open_us")
      stamp_read_us = field(stamp_detail, "read_us")
      stamp_checkpoint_read_us = field(stamp_detail, "checkpoint_read_us")
      stamp_compare_us = field(stamp_detail, "compare_us")
      stamp_checkpoint_compare_us = field(stamp_detail, "checkpoint_compare_us")
      unchanged_ms = ms_for("library_db_unchanged")
      if (first < 0) result = "missing_first_frame"
      else if (stamp_ms < 0) result = "missing_stamp_check"
      else if (stamp_check_us !~ /^[0-9]+$/) result = "invalid_stamp_check"
      else if (stamp_unchanged != "true") result = "catalog_changed"
      else if (unchanged_ms < stamp_ms) result = "missing_unchanged_terminal"
      else if ((stamp_check_us + 0) > validation_gate_us) result = "stamp_check_over_budget"
      else result = "ok"
      print label, iteration, first, first_ready, sync_ms, sync_total, summary_ms, summary_us, bridge_systems, bridge_sync, ready_ms, ready_load_us, load_open, load_schema, load_query, load_prepare, load_first_row, load_row_read, load_row_hydrate, load_launch_plans, load_systems, load_catalog, stamp_ms, stamp_unchanged, stamp_check_us, stamp_compute_us, stamp_open_us, stamp_read_us, stamp_checkpoint_read_us, stamp_compare_us, stamp_checkpoint_compare_us, unchanged_ms, result
    }
  ' "$log"
}

warm_catalog_self_test() {
  local tmp log row
  tmp="$(mktemp -d)"
  log="$tmp/warm.log"
  printf '%s\n' \
    $'startup_timing\tfirst_frame\t100ms\tcatalog_ready=true' \
    $'startup_timing\tcatalog_stamp_check\t9000ms\tunchanged=true check_us=2000000 compute_us=1 open_us=2 read_us=3 checkpoint_read_us=4 compare_us=5 checkpoint_compare_us=6' \
    $'startup_timing\tlibrary_db_unchanged\t9001ms\tscan_us=2000000' >"$log"
  row="$(warm_catalog_parse_log selftest 1 "$log")"
  [[ "${row##*$'\t'}" == "ok" ]]
  sed 's/check_us=2000000/check_us=2000001/' "$log" >"$tmp/over.log"
  row="$(warm_catalog_parse_log selftest 2 "$tmp/over.log")"
  [[ "${row##*$'\t'}" == "stamp_check_over_budget" ]]
  head -2 "$log" >"$tmp/missing-terminal.log"
  row="$(warm_catalog_parse_log selftest 3 "$tmp/missing-terminal.log")"
  [[ "${row##*$'\t'}" == "missing_unchanged_terminal" ]]
  head -1 "$log" >"$tmp/missing-check.log"
  row="$(warm_catalog_parse_log selftest 4 "$tmp/missing-check.log")"
  [[ "${row##*$'\t'}" == "missing_stamp_check" ]]
  sed 's/unchanged=true/unchanged=false/' "$log" >"$tmp/changed.log"
  row="$(warm_catalog_parse_log selftest 5 "$tmp/changed.log")"
  [[ "${row##*$'\t'}" == "catalog_changed" ]]
  rm -rf "$tmp"
  echo "profile-warm-catalog-start self-test ok"
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --self-test) warm_catalog_self_test; exit 0 ;;
    --replace-label) REPLACE_LABEL=1; shift ;;
    --iterations) ITERATIONS="${2:?}"; shift 2 ;;
    --deploy-device) DEPLOY=1; shift ;;
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
  LABEL="warmcat-$(date -u +%Y%m%dT%H%M%SZ)"
fi
if [[ ! "$LABEL" =~ ^[A-Za-z0-9_.-]+$ ]]; then
  echo "label must contain only letters, numbers, _, ., or -" >&2
  exit 2
fi
if [[ ! "$ITERATIONS" =~ ^[0-9]+$ || "$ITERATIONS" -lt 1 ]]; then
  echo "--iterations must be a positive integer" >&2
  exit 2
fi

mkdir -p "$BENCH_DIR" "$HERE/build/warm-catalog"
if [[ ! -f "$TSV" ]]; then
  printf '%s\n' "$TSV_HEADER" >"$TSV"
else
  tmp="$(mktemp)"
  python3 - "$TSV" "$TSV_HEADER" >"$tmp" <<'PY'
import csv
import sys

path = sys.argv[1]
header = sys.argv[2].split("\t")
old_header = [
    "label",
    "iteration",
    "first_frame_ms",
    "first_frame_catalog_ready",
    "catalog_cache_load_sync_ms",
    "catalog_cache_load_sync_total_us",
    "catalog_summary_load_ms",
    "catalog_summary_load_us",
    "catalog_bridge_systems_us",
    "catalog_bridge_sync_us",
    "full_catalog_ready_ms",
    "full_catalog_ready_load_us",
    "result",
]
with open(path, newline="") as f:
    rows = list(csv.reader(f, delimiter="\t"))
if not rows:
    print("\t".join(header))
    raise SystemExit(0)
source_header = rows[0]
print("\t".join(header))
for raw in rows[1:]:
    if len(source_header) == len(header):
        row = dict(zip(source_header, raw))
    elif len(source_header) == len(old_header):
        row = dict(zip(old_header, raw))
    else:
        row = dict(zip(source_header, raw))
    print("\t".join(row.get(column, "") for column in header))
PY
  mv "$tmp" "$TSV"
fi
if [[ "$REPLACE_LABEL" -eq 1 ]]; then
  tmp="$(mktemp)"
  { head -1 "$TSV"; grep -v "^${LABEL}	" "$TSV" | tail -n +2; } >"$tmp" || true
  mv "$tmp" "$TSV"
fi

if [[ "$DEPLOY" -eq 1 ]]; then
  "$HERE/scripts/deploy-rust.sh" --device --ui-scope launcher
fi

env_file="$(mktemp)"
cleanup() {
  rm -f "$env_file"
  "$MISTER" run "rm -f '$REMOTE_ENV'; if [ -p /dev/MiSTer_cmd ]; then printf 'mister_magik_restart_launcher\n' > /dev/MiSTer_cmd; fi" >/dev/null 2>&1 || true
}
trap cleanup EXIT

{
  printf 'export MISTER_CATALOG_REFRESH=default\n'
  printf 'export MISTER_LAUNCHER_START_SCREEN=home\n'
  printf 'export MISTER_LAUNCHER_LOCK_SCREEN=home\n'
} >"$env_file"

echo "== warm catalog startup profile label=$LABEL iterations=$ITERATIONS"
for ((iteration = 1; iteration <= ITERATIONS; iteration++)); do
  local_log="$HERE/build/warm-catalog/${LABEL}-${iteration}.log"
  "$MISTER" put "$env_file" "$REMOTE_ENV" >/dev/null
  "$MISTER" run "rm -f '$REMOTE_LOG'; if [ ! -p /dev/MiSTer_cmd ]; then echo 'missing /dev/MiSTer_cmd'; exit 12; fi; printf 'mister_magik_restart_launcher\n' > /dev/MiSTer_cmd" >/dev/null
  deadline=$((SECONDS + VALIDATION_TIMEOUT_SECS))
  while (( SECONDS < deadline )); do
    "$MISTER" get "$REMOTE_LOG" "$local_log" >/dev/null 2>&1 || true
    if grep -q $'^startup_timing\tlibrary_db_unchanged\t' "$local_log" 2>/dev/null ||
       grep -q $'^startup_timing\tlibrary_changed_detected\t' "$local_log" 2>/dev/null ||
       grep -q $'^startup_timing\tcatalog_stamp_check_failed\t' "$local_log" 2>/dev/null; then
      break
    fi
    sleep 1
  done
  "$MISTER" get "$REMOTE_LOG" "$local_log" >/dev/null 2>&1 || true
  if [[ ! -s "$local_log" ]]; then
    echo "warm catalog benchmark failed; missing launcher log $local_log" >&2
    exit 1
  fi
  row="$(warm_catalog_parse_log "$LABEL" "$iteration" "$local_log")"
  printf '%s\n' "$row" >>"$TSV"
  printf '%s\n' "$row"
  result="${row##*$'\t'}"
  if [[ "$result" != "ok" ]]; then
    echo "warm catalog validation failed: result=$result gate_us=$WARM_VALIDATION_GATE_US log=$local_log" >&2
    exit 1
  fi
done

echo "appended to $TSV"
