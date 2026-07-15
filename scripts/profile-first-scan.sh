#!/usr/bin/env bash
# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

# Profile the real first-boot library scan path on a MiSTer.
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
MISTER="$HERE/scripts/mister"
source "$HERE/scripts/magik-layout.sh"
magik_layout_select dev
REMOTE_BIN="$MISTER_MAGIK_BIN"
REMOTE_CATALOG_BUILDER="$MISTER_MAGIK_CATALOG_BUILDER"
REMOTE_LOG="/tmp/mister-magik-slint.log"
REMOTE_REFRESH_LOG="/tmp/mister-magik-library-refresh.log"
REMOTE_EVENTS="/tmp/mister-magik/events.jsonl"
REMOTE_DB="$MISTER_MAGIK_LIBRARY_DB"
REMOTE_SUMMARY="$MISTER_MAGIK_APP_DIR/library.summary.json"
REMOTE_NAV="$MISTER_MAGIK_APP_DIR/library.nav.lz4b"
REMOTE_ENV="$MISTER_MAGIK_LAUNCHER_ENV"
BENCH_DIR="$HERE/history/toolchain-bench"
OUT_DIR="$HERE/build/first-scan-profiles"
TSV="$BENCH_DIR/results-first-scan.tsv"
LABEL=""
DEPLOY="skip"
REPLACE_LABEL=0
TIMEOUT_SECS=240
SQLITE_BUILD_DIR=""
NAMESPACE_BACKEND=""
# Reference-MiSTer regression gates. They detect performance/content drift; they
# are not beta shipping blockers.
RAM_CATALOG_READY_GATE_MS=96592
DB_SAVE_GATE_MS=117766
CATALOG_GAME_COUNT_GATE=53457
CATALOG_SYSTEM_COUNT_GATE=71
CATALOG_DB_MAX_BYTES=13151232
source "$HERE/scripts/thread-sampler-lib.sh"
source "$HERE/scripts/mister-supervision-lib.sh"
source "$HERE/scripts/bench-context-lib.sh"
source "$HERE/scripts/benchmark-cleanup-lib.sh"

usage() {
  cat <<'EOF'
Usage: scripts/profile-first-scan.sh LABEL [--deploy-device|--deploy-catalog|--skip-build] [--replace-label] [--timeout SECS] [--sqlite-build-dir DIR] [--namespace-backend auto|walkdir|fd-relative] [--thread-sample]
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

first_scan_catalog_gate_check() {
  local games="$1" systems="$2" db_bytes="$3"
  [[ "$games" == "$CATALOG_GAME_COUNT_GATE" ]] || return 1
  [[ "$systems" == "$CATALOG_SYSTEM_COUNT_GATE" ]] || return 1
  [[ "$db_bytes" =~ ^[0-9]+$ ]] || return 1
  (( db_bytes <= CATALOG_DB_MAX_BYTES )) || return 1
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

first_scan_extract_complete_pair() {
  local builder_log="$1" launcher_log="$2" events_log="${3:-}"
  local ready_ms="" saved_ms="" ready_us="" saved_us="" event_pair="" event_pid="" event_boot_epoch_ms=""
  if [[ -f "$events_log" ]]; then
    if ! event_pair="$(first_scan_runtime_event_pair "$events_log")"; then
      return 1
    fi
    IFS=$'\t' read -r ready_ms saved_ms event_pid event_boot_epoch_ms <<<"$event_pair"
  fi
  if [[ -f "$builder_log" ]]; then
    ready_us="$(sed -n 's/.*"name":"builder_catalog_ready","detail":"elapsed_us=\([0-9][0-9]*\).*/\1/p' "$builder_log" | tail -1)"
    saved_us="$(sed -n 's/.*"name":"builder_persisted","detail":"elapsed_us=\([0-9][0-9]*\).*/\1/p' "$builder_log" | tail -1)"
  fi
  if [[ "$ready_ms" =~ ^[0-9]+$ && "$saved_ms" =~ ^[0-9]+$ ]]; then
    printf '%s\t%s\tlauncher-events\t%s\t%s\n' "$ready_ms" "$saved_ms" "${ready_us:-missing}" "${saved_us:-missing}"
    return 0
  fi
  ready_ms=""
  saved_ms=""
  if [[ -f "$launcher_log" ]]; then
    ready_ms="$(awk -F '\t' '$1 == "startup_timing" && $2 == "library_ready" { value=$3; sub(/ms$/, "", value); latest=value } END { print latest }' "$launcher_log")"
    saved_ms="$(awk -F '\t' '$1 == "startup_timing" && $2 == "library_db_saved" { value=$3; sub(/ms$/, "", value); latest=value } END { print latest }' "$launcher_log")"
  fi
  if [[ "$ready_ms" =~ ^[0-9]+$ && "$saved_ms" =~ ^[0-9]+$ ]]; then
    printf '%s\t%s\tlauncher-log\t%s\t%s\n' "$ready_ms" "$saved_ms" "${ready_us:-missing}" "${saved_us:-missing}"
    return 0
  fi
  if [[ "$ready_us" =~ ^[0-9]+$ && "$saved_us" =~ ^[0-9]+$ ]]; then
    printf '%s\t%s\tstandalone-builder\t%s\t%s\n' \
      "$(( (ready_us + 500) / 1000 ))" "$(( (saved_us + 500) / 1000 ))" "$ready_us" "$saved_us"
    return 0
  fi
  return 1
}

first_scan_runtime_event_pair() {
  local events_log="$1"
  python3 - "$events_log" <<'PY'
import json
import re
import sys

path = sys.argv[1]
relevant = []
try:
    source = open(path, encoding="utf-8", errors="replace")
except OSError:
    raise SystemExit(0)
with source:
    for line in source:
        try:
            row = json.loads(line)
            event = str(row.get("event") or "")
            detail = str(row.get("detail") or "")
            pid = int(row["pid"])
            boot_ms = int(row["ts_boot_ms"])
            unix_ms = int(row["ts_unix_ms"])
        except (KeyError, TypeError, ValueError):
            continue
        if event not in {"library_ready", "library_db_saved"}:
            continue
        match = re.search(r"(?:^|\s)since_run_ui_ms=(\d+)(?:\s|$)", detail)
        if match is None or pid <= 0 or boot_ms < 0 or unix_ms < boot_ms:
            continue
        relevant.append((event, int(match.group(1)), pid, boot_ms, unix_ms - boot_ms))

if not relevant:
    raise SystemExit(0)

pids = {row[2] for row in relevant}
boot_epochs = [row[4] for row in relevant]
if len(pids) != 1 or max(boot_epochs) - min(boot_epochs) > 2_000:
    raise SystemExit(2)

saved_rows = [row for row in relevant if row[0] == "library_db_saved"]
if not saved_rows:
    raise SystemExit(0)
saved = max(saved_rows, key=lambda row: row[3])
ready_rows = [
    row for row in relevant
    if row[0] == "library_ready" and row[3] <= saved[3] and row[1] <= saved[1]
]
if not ready_rows:
    raise SystemExit(0)
ready = max(ready_rows, key=lambda row: row[3])
boot_epoch_ms = round(sum(boot_epochs) / len(boot_epochs))
print(f"{ready[1]}\t{saved[1]}\t{ready[2]}\t{boot_epoch_ms}")
PY
}

first_scan_normalize_events() {
  local events_log="$1"
  python3 - "$events_log" <<'PY'
import json
import re
import sys

path = sys.argv[1]
try:
    source = open(path, encoding="utf-8", errors="replace")
except OSError:
    raise SystemExit(0)
with source:
    for line in source:
        try:
            row = json.loads(line)
        except (TypeError, ValueError):
            continue
        event = str(row.get("event") or "")
        detail = str(row.get("detail") or "")
        match = re.search(r"(?:^|\s)since_run_ui_ms=(\d+)(?:\s|$)", detail)
        if not event or match is None:
            continue
        print(f"startup_timing\t{event}\t{match.group(1)}ms\t{detail} source=runtime-events")
PY
}

first_scan_normalize_builder_timings() {
  local builder_log="$1"
  python3 - "$builder_log" <<'PY'
import json
import re
import sys

path = sys.argv[1]
metric_keys = {
    "builder_deferred_audit_stamp": "elapsed_us",
    "builder_catalog_projection": "elapsed_us",
    "builder_catalog_prepare_overlap": "wall_us",
}
try:
    source = open(path, encoding="utf-8", errors="replace")
except OSError:
    raise SystemExit(0)
with source:
    for line in source:
        try:
            row = json.loads(line)
        except (TypeError, ValueError):
            continue
        if row.get("event") != "timing":
            continue
        name = str(row.get("name") or "")
        metric_key = metric_keys.get(name)
        if metric_key is None:
            continue
        detail = str(row.get("detail") or "")
        match = re.search(rf"(?:^|\s){re.escape(metric_key)}=(\d+)(?:\s|$)", detail)
        if match is None:
            continue
        elapsed_us = int(match.group(1))
        elapsed_ms = (elapsed_us + 500) // 1000
        print(
            f"startup_timing\t{name}\t{elapsed_ms}ms\t"
            f"{detail} source=standalone-builder"
        )
PY
}

first_scan_write_canonical_log() {
  local output="$1" builder_log="$2" launcher_log="$3" events_log="${4:-}"
  local pair ready_ms saved_ms marker_source ready_us saved_us normalized_events normalized_builder_timings
  pair="$(first_scan_extract_complete_pair "$builder_log" "$launcher_log" "$events_log")" || return 1
  IFS=$'\t' read -r ready_ms saved_ms marker_source ready_us saved_us <<<"$pair"
  normalized_events="$(mktemp)"
  normalized_builder_timings="$(mktemp)"
  first_scan_normalize_events "$events_log" >"$normalized_events"
  first_scan_normalize_builder_timings "$builder_log" >"$normalized_builder_timings"
  {
    if [[ "$marker_source" == launcher-* ]]; then
      printf 'startup_timing\tlibrary_ready\t%sms\tsource=%s elapsed_ms=%s builder_elapsed_us=%s\n' "$ready_ms" "$marker_source" "$ready_ms" "$ready_us"
      printf 'startup_timing\tlibrary_db_saved\t%sms\tsource=%s elapsed_ms=%s builder_elapsed_us=%s\n' "$saved_ms" "$marker_source" "$saved_ms" "$saved_us"
    else
      printf 'startup_timing\tlibrary_ready\t%sms\tsource=standalone-builder elapsed_us=%s\n' "$ready_ms" "$ready_us"
      printf 'startup_timing\tlibrary_db_saved\t%sms\tsource=standalone-builder elapsed_us=%s\n' "$saved_ms" "$saved_us"
    fi
    cat "$normalized_builder_timings"
    awk -F '\t' '!($1 == "startup_timing" && ($2 == "library_ready" || $2 == "library_db_saved" || $2 == "builder_deferred_audit_stamp" || $2 == "builder_catalog_projection" || $2 == "builder_catalog_prepare_overlap"))' "$builder_log" "$launcher_log" "$normalized_events"
  } >"$output"
  rm -f "$normalized_events" "$normalized_builder_timings"
}

first_scan_marker_self_test() {
  local tmp builder launcher events combined pair
  tmp="$(mktemp -d)"
  builder="$tmp/builder.log"
  launcher="$tmp/launcher.log"
  events="$tmp/events.jsonl"
  combined="$tmp/combined.log"
  printf '%s\n' '{"event":"timing","name":"builder_catalog_ready","detail":"elapsed_us=1000500"}' \
    '{"event":"timing","name":"builder_deferred_audit_stamp","detail":"elapsed_us=12500 audit_us=11000 stamp_us=1500 audit_rows=4"}' \
    '{"event":"timing","name":"builder_catalog_projection","detail":"elapsed_us=23500 games=10"}' \
    '{"event":"timing","name":"builder_catalog_prepare_overlap","detail":"wall_us=24000 audit_stamp_worker_us=12500 audit_us=11000 stamp_us=1500 catalog_us=23500 overlapped_us=12000 mode=scoped-dual-core"}' \
    '{"event":"timing","name":"builder_persisted","detail":"elapsed_us=2000500"}' >"$builder"
  : >"$launcher"
  pair="$(first_scan_extract_complete_pair "$builder" "$launcher" "$events")"
  [[ "$pair" == $'1001\t2001\tstandalone-builder\t1000500\t2000500' ]]
  printf 'startup_timing\tlibrary_ready\t1100ms\tgames=10\nstartup_timing\tlibrary_db_saved\t2200ms\tbytes=20\n' >"$launcher"
  pair="$(first_scan_extract_complete_pair "$builder" "$launcher" "$events")"
  [[ "$pair" == $'1100\t2200\tlauncher-log\t1000500\t2000500' ]]
  printf '%s\n' \
    '{"ts_unix_ms":101200,"ts_boot_ms":1200,"pid":42,"event":"library_ready","detail":"since_run_ui_ms=1200 games=10"}' \
    '{"ts_unix_ms":102400,"ts_boot_ms":2400,"pid":42,"event":"library_db_saved","detail":"since_run_ui_ms=2400 bytes=20"}' >"$events"
  pair="$(first_scan_extract_complete_pair "$builder" "$launcher" "$events")"
  [[ "$pair" == $'1200\t2400\tlauncher-events\t1000500\t2000500' ]]
  first_scan_write_canonical_log "$combined" "$builder" "$launcher" "$events"
  [[ "$(grep -c $'^startup_timing\tlibrary_ready\t' "$combined")" == "1" ]]
  [[ "$(grep -c $'^startup_timing\tlibrary_db_saved\t' "$combined")" == "1" ]]
  [[ "$(grep -c $'^startup_timing\tbuilder_deferred_audit_stamp\t' "$combined")" == "1" ]]
  [[ "$(grep -c $'^startup_timing\tbuilder_catalog_projection\t' "$combined")" == "1" ]]
  [[ "$(grep -c $'^startup_timing\tbuilder_catalog_prepare_overlap\t' "$combined")" == "1" ]]
  grep -q $'^startup_timing\tlibrary_ready\t1200ms\tsource=launcher-events elapsed_ms=1200 builder_elapsed_us=1000500$' "$combined"
  grep -q $'^startup_timing\tbuilder_deferred_audit_stamp\t13ms\telapsed_us=12500 .*source=standalone-builder$' "$combined"
  grep -q $'^startup_timing\tbuilder_catalog_projection\t24ms\telapsed_us=23500 .*source=standalone-builder$' "$combined"
  grep -q $'^startup_timing\tbuilder_catalog_prepare_overlap\t24ms\twall_us=24000 .*source=standalone-builder$' "$combined"
  if grep -q $'source=launcher-.* elapsed_us=' "$combined"; then
    rm -rf "$tmp"
    echo "first-scan canonical log mislabeled builder microseconds as the launcher clock" >&2
    return 1
  fi
  printf '%s\n' \
    '{"ts_unix_ms":101200,"ts_boot_ms":1200,"pid":42,"event":"library_ready","detail":"since_run_ui_ms=1200 games=10"}' \
    '{"ts_unix_ms":102400,"ts_boot_ms":2400,"pid":43,"event":"library_db_saved","detail":"since_run_ui_ms=2400 bytes=20"}' >"$events"
  if first_scan_extract_complete_pair "$builder" "$launcher" "$events" >/dev/null; then
    rm -rf "$tmp"
    echo "first-scan marker parser accepted runtime events from two launcher PIDs" >&2
    return 1
  fi
  printf '%s\n' \
    '{"ts_unix_ms":101200,"ts_boot_ms":1200,"pid":42,"event":"library_ready","detail":"since_run_ui_ms=1200 games=10"}' \
    '{"ts_unix_ms":202400,"ts_boot_ms":2400,"pid":42,"event":"library_db_saved","detail":"since_run_ui_ms=2400 bytes=20"}' >"$events"
  if first_scan_extract_complete_pair "$builder" "$launcher" "$events" >/dev/null; then
    rm -rf "$tmp"
    echo "first-scan marker parser accepted runtime events from two boot epochs" >&2
    return 1
  fi
  printf 'startup_timing\tlibrary_ready\t1100ms\tgames=10\n' >"$launcher"
  : >"$events"
  pair="$(first_scan_extract_complete_pair "$builder" "$launcher" "$events")"
  [[ "$pair" == $'1001\t2001\tstandalone-builder\t1000500\t2000500' ]]
  printf '%s\n' '{"event":"timing","name":"builder_catalog_ready","detail":"elapsed_us=1000500"}' >"$builder"
  if first_scan_extract_complete_pair "$builder" "$launcher" "$events" >/dev/null; then
    rm -rf "$tmp"
    echo "first-scan marker parser accepted two incomplete clocks" >&2
    return 1
  fi
  rm -rf "$tmp"
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
  first_scan_catalog_gate_check \
    "$CATALOG_GAME_COUNT_GATE" \
    "$CATALOG_SYSTEM_COUNT_GATE" \
    "$CATALOG_DB_MAX_BYTES"
  if first_scan_catalog_gate_check \
    "$CATALOG_GAME_COUNT_GATE" \
    "$CATALOG_SYSTEM_COUNT_GATE" \
    $((CATALOG_DB_MAX_BYTES + 1)); then
    echo "catalog size gate accepted max+1" >&2
    return 1
  fi
  if first_scan_catalog_gate_check \
    $((CATALOG_GAME_COUNT_GATE - 1)) \
    "$CATALOG_SYSTEM_COUNT_GATE" \
    "$CATALOG_DB_MAX_BYTES"; then
    echo "catalog game-count gate accepted the wrong count" >&2
    return 1
  fi
  if first_scan_catalog_gate_check \
    "$CATALOG_GAME_COUNT_GATE" \
    $((CATALOG_SYSTEM_COUNT_GATE - 1)) \
    "$CATALOG_DB_MAX_BYTES"; then
    echo "catalog system-count gate accepted the wrong count" >&2
    return 1
  fi
  if first_scan_catalog_gate_check \
    "$CATALOG_GAME_COUNT_GATE" \
    "$CATALOG_SYSTEM_COUNT_GATE" \
    ""; then
    echo "catalog size gate accepted a missing size" >&2
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
  first_scan_marker_self_test
  first_scan_thread_sample_self_test
  first_scan_identity_self_test
  first_scan_reset_artifact_self_test
  echo "profile-first-scan self-test ok"
}

first_scan_thread_sample_process() {
  printf 'mister-magik-catalog-builder\n'
}

first_scan_thread_sample_has_builder_evidence() {
  local path="$1"
  [[ -s "$path" ]] &&
    awk -F '\t' '
      $1 == "thread_sample_tsv" && $2 != "sample" &&
      $6 ~ /^[0-9]+$/ && $10 ~ /^[0-9]+$/ &&
      $20 ~ /^[0-9]+$/ && $20 + 0 > 0 {
        found = 1
      }
      END { exit(found ? 0 : 1) }
    ' "$path"
}

first_scan_has_catalog_audit_policy_evidence() {
  local path="$1"
  [[ -s "$path" ]] &&
    awk -F '\t' '
      $1 == "thread_policy_tsv" &&
      $2 == "thread=catalog-audit" &&
      $3 == "role=catalog-foreground" &&
      $4 == "intended_nice=0" &&
      $5 == "actual_nice=0" &&
      $6 == "affinity=all-online" &&
      $7 == "allowed_cpus=0-1" &&
      $8 ~ /^processor=[0-9]+$/ &&
      $9 == "nice_status=ok" &&
      $10 == "affinity_status=ok" {
        found = 1
      }
      END { exit(found ? 0 : 1) }
    ' "$path"
}

first_scan_thread_sample_self_test() {
  local tmp sample policy
  [[ "$(first_scan_thread_sample_process)" == "mister-magik-catalog-builder" ]]
  tmp="$(mktemp -d)"
  sample="$tmp/sample.tsv"
  printf '%s\n' \
    $'thread_sample_tsv\tsample\tts_unix\tinterval_start_monotonic_us\tmonotonic_us\tpid\ttid\tthread_name\tstate\tprocessor\tutime_jiffies\tstime_jiffies\tutime_delta_jiffies\tstime_delta_jiffies\tvoluntary_ctxt_switches\tnonvoluntary_ctxt_switches\tvoluntary_delta\tnonvoluntary_delta\tvmrss_kb\tvmhwm_kb' \
    $'thread_sample_tsv\t0\t1\t1\t1\t42\t42\tmister-magik-c\tR\t1\t1\t0\t0\t0\t1\t0\t0\t0\t64000\t64000' >"$sample"
  first_scan_thread_sample_has_builder_evidence "$sample"
  sed 's/\t1\t1\t0\t0\t0\t1\t0\t0\t0\t64000\t64000$/\tnot-a-cpu\t1\t0\t0\t0\t1\t0\t0\t0\t64000\t64000/' "$sample" >"$tmp/bad-cpu.tsv"
  if first_scan_thread_sample_has_builder_evidence "$tmp/bad-cpu.tsv"; then
    rm -rf "$tmp"
    echo "first-scan thread evidence accepted a non-numeric processor" >&2
    return 1
  fi
  sed 's/\t64000$/\t0/' "$sample" >"$tmp/missing-hwm.tsv"
  if first_scan_thread_sample_has_builder_evidence "$tmp/missing-hwm.tsv"; then
    rm -rf "$tmp"
    echo "first-scan thread evidence accepted missing peak RSS" >&2
    return 1
  fi
  head -1 "$sample" >"$tmp/empty.tsv"
  if first_scan_thread_sample_has_builder_evidence "$tmp/empty.tsv"; then
    rm -rf "$tmp"
    echo "first-scan thread evidence accepted a header-only sample" >&2
    return 1
  fi
  policy="$tmp/catalog-audit.log"
  printf '%s\n' \
    $'thread_policy_tsv\tthread=catalog-audit\trole=catalog-foreground\tintended_nice=0\tactual_nice=0\taffinity=all-online\tallowed_cpus=0-1\tprocessor=1\tnice_status=ok\taffinity_status=ok' >"$policy"
  first_scan_has_catalog_audit_policy_evidence "$policy"
  sed 's/actual_nice=0/actual_nice=10/' "$policy" >"$tmp/bad-nice.log"
  if first_scan_has_catalog_audit_policy_evidence "$tmp/bad-nice.log"; then
    rm -rf "$tmp"
    echo "first-scan policy evidence accepted background nice" >&2
    return 1
  fi
  sed 's/allowed_cpus=0-1/allowed_cpus=0/' "$policy" >"$tmp/bad-affinity.log"
  if first_scan_has_catalog_audit_policy_evidence "$tmp/bad-affinity.log"; then
    rm -rf "$tmp"
    echo "first-scan policy evidence accepted single-core affinity" >&2
    return 1
  fi
  rm -rf "$tmp"
}

first_scan_identity_self_test() {
  local tmp binary hash bad_hash
  tmp="$(mktemp -d)"
  binary="$tmp/mister-magik-fb"
  printf 'binary\n' >"$binary"
  printf 'ui\n' >"$binary.features"
  bench_context_write_build_receipt "$binary" "$HERE" release-device ui launcher
  hash="$(bench_context_sha256_file "$binary")"
  bench_context_require_binary_contract "$binary" "$hash" ui release-device launcher
  if bench_context_require_binary_contract "$binary" "$hash" builder release-device launcher; then
    rm -rf "$tmp"
    echo "first-scan identity accepted the wrong feature contract" >&2
    return 1
  fi
  bad_hash="${hash%?}$([[ "${hash: -1}" == "0" ]] && printf '1' || printf '0')"
  if bench_context_require_binary_contract "$binary" "$bad_hash" ui release-device launcher; then
    rm -rf "$tmp"
    echo "first-scan identity accepted a deployed hash mismatch" >&2
    return 1
  fi
  rm -rf "$tmp"
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
    --namespace-backend) NAMESPACE_BACKEND="${2:?}"; shift 2 ;;
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
if [[ -n "$NAMESPACE_BACKEND" &&
      "$NAMESPACE_BACKEND" != "auto" &&
      "$NAMESPACE_BACKEND" != "walkdir" &&
      "$NAMESPACE_BACKEND" != "fd-relative" ]]; then
  echo "--namespace-backend must be auto, walkdir, or fd-relative" >&2
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
local_events="$(mktemp)"
combined_log="$(mktemp)"
raw_log="$OUT_DIR/${LABEL}-launcher.log"
raw_refresh_log="$OUT_DIR/${LABEL}-catalog-builder.log"
raw_events="$OUT_DIR/${LABEL}-events.jsonl"
artifact_report="$OUT_DIR/${LABEL}-artifacts.tsv"
cleanup_report="$OUT_DIR/${LABEL}-cleanup.txt"
launcher_suspended=0
binary_path="$HERE/magik-gui/target/armv7-unknown-linux-gnueabihf/release-device/mister-magik-fb"
catalog_builder_path="$HERE/magik-gui/target/armv7-unknown-linux-gnueabihf/release-device/mister-magik-catalog-builder"
deployment_state="verified"
deployed_sha256="$(bench_context_remote_sha256 "$MISTER" "$REMOTE_BIN" || true)"
deployed_sha256="${deployed_sha256:-missing}"
local_sha256="$(bench_context_sha256_file "$binary_path")"
if ! bench_context_require_binary_contract "$binary_path" "$deployed_sha256" ui release-device launcher; then
  echo "first-scan launcher identity verification failed local=$local_sha256 deployed=$deployed_sha256 features=$(bench_context_binary_features "$binary_path") expected_features=ui" >&2
  exit 1
fi
catalog_builder_deployed_sha256="$(bench_context_remote_sha256 "$MISTER" "$REMOTE_CATALOG_BUILDER" || true)"
catalog_builder_deployed_sha256="${catalog_builder_deployed_sha256:-missing}"
catalog_builder_local_sha256="$(bench_context_sha256_file "$catalog_builder_path")"
if ! bench_context_require_binary_contract "$catalog_builder_path" "$catalog_builder_deployed_sha256" builder release-device all; then
  echo "first-scan catalog-builder identity verification failed local=$catalog_builder_local_sha256 deployed=$catalog_builder_deployed_sha256 features=$(bench_context_binary_features "$catalog_builder_path") expected_features=builder" >&2
  exit 1
fi
binary_fields="$(bench_context_binary_fields release-device launcher ui "$binary_path" production "$deployment_state" "$deployed_sha256")"
catalog_builder_fields="$(bench_context_binary_fields release-device all builder "$catalog_builder_path" production "$deployment_state" "$catalog_builder_deployed_sha256")"
source_fields="$(bench_context_source_fields "$HERE")"
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
  printf 'artifact_tsv\tlabel=%s\tkind=runtime_events\tlocal_path=%s\tremote_path=%s\texists=%s\tbytes=%s\n' \
    "$LABEL" "$raw_events" "$REMOTE_EVENTS" "$([[ -f "$raw_events" ]] && echo true || echo false)" "$([[ -f "$raw_events" ]] && wc -c <"$raw_events" | tr -d ' ' || echo 0)"
  printf 'run_context_tsv\tlabel=%s\tcommit=%s\tdeploy=%s\t%s\t%s\n' "$LABEL" "$commit" "$DEPLOY" "$source_fields" "$binary_fields"
  printf 'binary_context_tsv\tlabel=%s\trole=catalog-builder\t%s\n' "$LABEL" "$catalog_builder_fields"
  if [[ "$thread_sample_enabled" == "1" ]]; then
    thread_sample_emit_artifacts
    thread_sample_emit_summary "$LABEL" "first-scan-builder" "$thread_sample_local_tsv"
  fi
}
profile_first_scan_cleanup() {
  local cleanup_status=0
  rm -f "$local_log" "$local_refresh_log" "$local_events" "$combined_log" "$env_file"
  if [[ "$launcher_suspended" == "1" ]]; then
    mister_supervision_command "mister_magik_resume" 0.5 >/dev/null 2>&1 || cleanup_status=1
    launcher_suspended=0
  fi
  benchmark_cleanup_clear_launcher_env "$MISTER" 30 >/dev/null 2>&1 || cleanup_status=1
  if benchmark_cleanup_assert_no_arming_files "$MISTER" "$cleanup_report"; then
    printf 'cleanup_tsv\tlabel=%s\tvalid=1\tinvalid_reason=ok\n' "$LABEL" >>"$artifact_report"
  else
    printf 'cleanup_tsv\tlabel=%s\tvalid=0\tinvalid_reason=stale-arming-or-device-error\tdetail=%s\n' "$LABEL" "$cleanup_report" >>"$artifact_report"
    cleanup_status=1
  fi
  return "$cleanup_status"
}
benchmark_cleanup_install profile_first_scan_cleanup
: >"$env_file"
if [[ -n "$SQLITE_BUILD_DIR" ]]; then
  printf 'export MISTER_LIBRARY_SQLITE_BUILD_DIR=%q\n' "$SQLITE_BUILD_DIR" >>"$env_file"
fi
if [[ -n "$NAMESPACE_BACKEND" ]]; then
  printf 'export MISTER_LIBRARY_NAMESPACE_BACKEND=%q\n' "$NAMESPACE_BACKEND" >>"$env_file"
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
rm -f '$REMOTE_DB' '$REMOTE_SUMMARY' '$REMOTE_NAV' '$REMOTE_LOG' '$REMOTE_REFRESH_LOG' '$REMOTE_EVENTS'
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
thread_sample_start \
  "$LABEL" \
  "first-scan" \
  "$OUT_DIR" \
  "$TIMEOUT_SECS" \
  "$(first_scan_thread_sample_process)"

deadline=$((SECONDS + TIMEOUT_SECS))
while (( SECONDS < deadline )); do
  "$MISTER" get "$REMOTE_REFRESH_LOG" "$local_refresh_log" >/dev/null 2>&1 || true
  "$MISTER" get "$REMOTE_LOG" "$local_log" >/dev/null 2>&1 || true
  "$MISTER" get "$REMOTE_EVENTS" "$local_events" >/dev/null 2>&1 || true
  if grep -q '"event":"failure"' "$local_refresh_log" ||
     grep -q $'^startup_timing\tlibrary_db_save_failed\t' "$local_log"; then
    break
  fi
  if first_scan_extract_complete_pair "$local_refresh_log" "$local_log" "$local_events" >/dev/null; then
    break
  fi
  sleep 2
done

"$MISTER" get "$REMOTE_REFRESH_LOG" "$local_refresh_log" >/dev/null 2>&1 || true
"$MISTER" get "$REMOTE_LOG" "$local_log" >/dev/null 2>&1 || true
"$MISTER" get "$REMOTE_EVENTS" "$local_events" >/dev/null 2>&1 || true
cp "$local_log" "$raw_log"
cp "$local_refresh_log" "$raw_refresh_log"
cp "$local_events" "$raw_events"
thread_sample_stop
thread_sample_collect
if [[ "$thread_sample_enabled" == "1" ]] &&
   { ! first_scan_thread_sample_has_builder_evidence "$thread_sample_local_tsv" ||
     ! first_scan_has_catalog_audit_policy_evidence "$local_refresh_log"; }; then
  emit_thread_sample_artifact_report | tee "$artifact_report" || true
  echo "first-scan evidence did not prove catalog-builder CPU/HWM and catalog-audit foreground all-core policy" >&2
  exit 1
fi
if grep -q '"event":"failure"' "$local_refresh_log" ||
   grep -q $'^startup_timing\tlibrary_db_save_failed\t' "$local_log"; then
  emit_thread_sample_artifact_report | tee "$artifact_report" || true
  echo "first scan failed while saving the catalog; latest log follows" >&2
  tail -80 "$local_log" >&2 || true
  exit 1
fi
if marker_pair="$(first_scan_extract_complete_pair "$local_refresh_log" "$local_log" "$local_events")"; then
  IFS=$'\t' read -r ready_ms saved_ms marker_source ready_us saved_us <<<"$marker_pair"
  first_scan_write_canonical_log "$combined_log" "$local_refresh_log" "$local_log" "$local_events"
else
  ready_ms=""
  saved_ms=""
fi
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
  $1 == "startup_timing" && ($2 == "first_frame" || $2 == "bootstrap_counter_climb" || $2 == "bootstrap_counter_sustained_climb" || $2 == "full_scan_counter_climb" || $2 == "catalog_counter_climb" || $2 == "library_scan_complete" || $2 == "library_db_saved" || $2 == "library_ready" || $2 == "catalog_bridge_sync_update" || $2 == "catalog_worker_ram_catalog" || $2 == "builder_deferred_audit_stamp" || $2 == "builder_catalog_projection" || $2 == "builder_catalog_prepare_overlap") {
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

catalog_metrics="$("$MISTER" db "SELECT 'games' AS metric, count(*) AS value FROM games UNION ALL SELECT 'systems', count(*) FROM systems;" 2>/dev/null || true)"
catalog_games="$(printf '%s\n' "$catalog_metrics" | awk -F '\t' '$1 == "games" { print $2; exit }' | tr -d '\r')"
catalog_systems="$(printf '%s\n' "$catalog_metrics" | awk -F '\t' '$1 == "systems" { print $2; exit }' | tr -d '\r')"
catalog_db_bytes="$(printf '%s\n' "$catalog_metrics" | awk -F '\t' '$1 == "library_sql_timing_tsv" { print $3; exit }' | tr -d '\r')"
status="$("$MISTER" status 2>/dev/null || true)"
printf '%s\t%s\t%s\t%s\t%s\n' "$LABEL" "$commit" "catalog_games" "0" "$catalog_games" >>"$TSV"
printf '%s\t%s\t%s\t%s\t%s\n' "$LABEL" "$commit" "catalog_systems" "0" "$catalog_systems" >>"$TSV"
printf '%s\t%s\t%s\t%s\t%s\n' "$LABEL" "$commit" "catalog_db_bytes" "0" "$catalog_db_bytes" >>"$TSV"
if ! first_scan_catalog_gate_check "$catalog_games" "$catalog_systems" "$catalog_db_bytes"; then
  gate_failed=1
  echo "first scan catalog regression gate failed: games=${catalog_games:-missing}/${CATALOG_GAME_COUNT_GATE} systems=${catalog_systems:-missing}/${CATALOG_SYSTEM_COUNT_GATE} db_bytes=${catalog_db_bytes:-missing}/${CATALOG_DB_MAX_BYTES}" >&2
fi
emit_thread_sample_artifact_report | tee "$artifact_report"

echo "appended to $TSV"
echo "catalog_games=$catalog_games catalog_systems=$catalog_systems catalog_db_bytes=$catalog_db_bytes"
printf '%s\n' "$status"
if [[ "$gate_failed" -eq 1 ]]; then
  exit 1
fi
