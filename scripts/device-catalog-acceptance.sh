#!/usr/bin/env bash
# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
MISTER="${MISTER:-$ROOT/scripts/mister}"
source "$ROOT/scripts/lib/magik-layout.sh"
magik_layout_select dev
source "$ROOT/scripts/lib/bench-context-lib.sh"
REMOTE_BIN="$MISTER_MAGIK_BIN"
REMOTE_DB="$MISTER_MAGIK_LIBRARY_DB"
REMOTE_SUMMARY="$MISTER_MAGIK_APP_DIR/library.summary.json"
REMOTE_NAVIGATION="$MISTER_MAGIK_APP_DIR/library.nav.lz4b"
REMOTE_ASSETS="$MISTER_MAGIK_ASSET_DIR"
CATALOG_FIXTURE_CONTRACT="$ROOT/scripts/lib/catalog-fixture-contract.json"
CATALOG_FIXTURE_TOOL="$ROOT/scripts/lib/catalog-fixture-contract.py"
SETTLE_SECS=5
RACE_REFRESH=0
LABEL=""
REPLACE_LABEL=0
SELF_TEST=0
RESULTS_TSV=""

usage() {
  cat <<'USAGE'
usage: scripts/device-catalog-acceptance.sh [--settle SECS] [--race-refresh] [--label LABEL] [--replace-label] [--self-test]

Checks the deployed MiSTer catalog state through scripts/mister:
  - exactly one launcher process
  - no active library-refresh after settling
  - non-empty library.sqlite3
  - current generic catalog facts contain games and discoveries
  - current summary and navigation projections are present
  - production navigation filter options match full SQLite hydration
  - aggregate Arcade exposes more than two normalized categories
  - screenshot packs remain runtime-only and are not indexed into asset tables
  - optional duplicate refresh race proves one refresh skips via single-flight
USAGE
}

while [ "$#" -gt 0 ]; do
  case "$1" in
    --settle)
      SETTLE_SECS="${2:?--settle needs seconds}"
      shift 2
      ;;
    --race-refresh)
      RACE_REFRESH=1
      shift
      ;;
    --label)
      LABEL="${2:?--label needs a value}"
      shift 2
      ;;
    --replace-label)
      REPLACE_LABEL=1
      shift
      ;;
    --self-test)
      SELF_TEST=1
      shift
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      echo "ERROR: unknown argument: $1" >&2
      usage >&2
      exit 2
      ;;
  esac
done

remote() {
  "$MISTER" run "$1"
}

db() {
  "$MISTER" db "$1"
}

last_number() {
  awk 'NF { value=$NF } END { gsub(/[^0-9]/, "", value); print value }'
}

first_result_number() {
  awk '
    /^library_sql_timing_tsv[[:space:]]/ { next }
    NF && seen_header {
      value=$1
      gsub(/[^0-9]/, "", value)
      print value
      exit
    }
    NF { seen_header=1 }
  '
}

db_scalar() {
  db "$1" | first_result_number
}

result_value() {
  printf '%s' "$1" | tr '\t\r\n' '   '
}

acceptance_running_binary_matches() {
  local local_sha="$1" deployed_sha="$2" running_sha="$3"
  [[ -n "$local_sha" && "$local_sha" != "missing" && "$local_sha" == "$deployed_sha" && "$local_sha" == "$running_sha" ]]
}

record_result() {
  local check="$1" status="$2" expected="${3:-}" actual="${4:-}" detail="${5:-}"
  [[ -n "$RESULTS_TSV" ]] || return 0
  printf '%s\t%s\t%s\t%s\t%s\t%s\n' "$LABEL" "$(result_value "$check")" "$status" \
    "$(result_value "$expected")" "$(result_value "$actual")" "$(result_value "$detail")" >>"$RESULTS_TSV"
}

write_acceptance_summary() {
  local path="$1" final_status="$2" passed failed
  passed="$(awk -F '\t' 'NR > 1 && $3 == "pass" { count++ } END { print count + 0 }' "$RESULTS_TSV")"
  failed="$(awk -F '\t' 'NR > 1 && $3 == "fail" { count++ } END { print count + 0 }' "$RESULTS_TSV")"
  printf '{\n  "label": "%s",\n  "status": "%s",\n  "passed": %s,\n  "failed": %s\n}\n' \
    "$(result_value "$LABEL")" "$final_status" "$passed" "$failed" >"$path"
}

acceptance_reporting_self_test() {
  local tmp summary binary hash bad_hash
  tmp="$(mktemp -d)"
  RESULTS_TSV="$tmp/results.tsv"
  summary="$tmp/summary.json"
  LABEL="selftest"
  printf 'label\tcheck\tstatus\texpected\tactual\tdetail\n' >"$RESULTS_TSV"
  record_result "one" pass "1" "1" "ok"
  record_result "two" fail ">0" "0" "bad"
  write_acceptance_summary "$summary" FAIL
  grep -q '"passed": 1' "$summary"
  grep -q '"failed": 1' "$summary"
  grep -q $'selftest\ttwo\tfail\t>0\t0\tbad' "$RESULTS_TSV"
  python3 "$CATALOG_FIXTURE_TOOL" self-test
  binary="$tmp/mister-magik-fb"
  printf 'binary\n' >"$binary"
  printf 'ui\n' >"$binary.features"
  bench_context_write_build_receipt "$binary" "$ROOT" release-device ui launcher
  hash="$(bench_context_sha256_file "$binary")"
  bad_hash="${hash%?}$([[ "${hash: -1}" == "0" ]] && printf '1' || printf '0')"
  bench_context_require_binary_contract "$binary" "$hash" ui release-device launcher
  acceptance_running_binary_matches "$hash" "$hash" "$hash"
  if acceptance_running_binary_matches "$hash" "$hash" "$bad_hash"; then
    echo "catalog acceptance identity accepted a stale running inode" >&2
    rm -rf "$tmp"
    return 1
  fi
  if bench_context_require_binary_contract "$binary" "$hash" ui,bench-tools release-device launcher; then
    echo "catalog acceptance identity accepted the wrong feature contract" >&2
    rm -rf "$tmp"
    return 1
  fi
  rm -rf "$tmp"
  echo "device-catalog-acceptance self-test ok"
}

assert_eq() {
  local label="$1" expected="$2" actual="$3"
  if [ "$actual" != "$expected" ]; then
    record_result "$label" fail "$expected" "$actual"
    echo "FAIL: $label expected=$expected actual=$actual" >&2
    exit 1
  fi
  record_result "$label" pass "$expected" "$actual"
  echo "ok: $label = $actual"
}

assert_gt_zero() {
  local label="$1" actual="$2"
  if [ -z "$actual" ] || [ "$actual" -le 0 ]; then
    record_result "$label" fail ">0" "${actual:-empty}"
    echo "FAIL: $label expected > 0 actual=${actual:-empty}" >&2
    exit 1
  fi
  record_result "$label" pass ">0" "$actual"
  echo "ok: $label = $actual"
}

assert_gt() {
  local label="$1" minimum="$2" actual="$3"
  if [ -z "$actual" ] || [ "$actual" -le "$minimum" ]; then
    record_result "$label" fail ">$minimum" "${actual:-empty}"
    echo "FAIL: $label expected > $minimum actual=${actual:-empty}" >&2
    exit 1
  fi
  record_result "$label" pass ">$minimum" "$actual"
  echo "ok: $label = $actual"
}

assert_remote_nonempty() {
  local path="$1"
  if remote "test -s '$path'"; then
    record_result "$path non-empty" pass "true" "true"
    echo "ok: $path is present and non-empty"
  else
    record_result "$path non-empty" fail "true" "false"
    echo "FAIL: $path is missing or empty" >&2
    exit 1
  fi
}

catalog_filter_report() {
  local prefix="/tmp/mister-magik-catalog-filter-acceptance"
  local out="$prefix.tsv" err="$prefix.err" rc_file="$prefix.rc" pid state rc
  pid="$(
    remote "rm -f '$out' '$err' '$rc_file'; ( '$REMOTE_BIN' catalog-inspect filter-options menu:arcade >'$out' 2>'$err'; echo \$? >'$rc_file' ) & echo \$!" |
      last_number
  )"
  for _ in $(seq 1 60); do
    state="$(remote "if test -s '$rc_file'; then cat '$rc_file'; else echo running; fi" | awk 'NF { value=$NF } END { print value }')"
    if [ "$state" != "running" ]; then
      rc="$state"
      break
    fi
    sleep 1
  done
  if [ -z "${rc:-}" ]; then
    remote "kill '$pid' 2>/dev/null || true"
    echo "FAIL: catalog filter inspection timed out after 60 seconds" >&2
    return 1
  fi
  if [ "$rc" != "0" ]; then
    remote "cat '$err' 2>/dev/null || true" >&2
    echo "FAIL: catalog filter inspection exited rc=$rc" >&2
    return 1
  fi
  remote "cat '$out'"
}

pack_exists() {
  remote "test -f '$REMOTE_ASSETS/$1' && echo yes || echo no" | awk 'NF { value=$NF } END { print value }'
}

pack_exists_for_platform() {
  local platform="$1"
  remote "if test -f '$REMOTE_ASSETS/${platform}-screenshots-320x320.mmlz4b' || test -f '$REMOTE_ASSETS/${platform}-screenshots.mmlz4b'; then echo yes; else echo no; fi" | awk 'NF { value=$NF } END { print value }'
}

arcade_pack_exists() {
  pack_exists_for_platform arcade
}

if [ "$SELF_TEST" -eq 1 ]; then
  acceptance_reporting_self_test
  exit 0
fi

if [ -z "$LABEL" ]; then
  LABEL="catalog-acceptance-$(date -u +%Y%m%dT%H%M%SZ)"
fi
if [[ ! "$LABEL" =~ ^[A-Za-z0-9_.-]+$ ]]; then
  echo "ERROR: label must contain only letters, numbers, _, ., or -" >&2
  exit 2
fi
OUT="$ROOT/build/catalog-acceptance/$LABEL"
if [ -e "$OUT/results.tsv" ] && [ "$REPLACE_LABEL" -ne 1 ]; then
  echo "ERROR: acceptance artifacts already exist for $LABEL; pass --replace-label" >&2
  exit 2
fi
mkdir -p "$OUT"
RESULTS_TSV="$OUT/results.tsv"
REPORT_LOG="$OUT/report.log"
FINAL_SUMMARY="$OUT/summary.json"
SUMMARY_COPY="$OUT/catalog-summary.json"
STATUS_JSON="$OUT/status.json"
RUN_CONTEXT="$OUT/run-context.tsv"
printf 'label\tcheck\tstatus\texpected\tactual\tdetail\n' >"$RESULTS_TSV"
: >"$REPORT_LOG"
acceptance_complete=0
acceptance_finish() {
  local status=$? final_status="FAIL"
  trap - EXIT
  set +e
  "$MISTER" status --json >"$STATUS_JSON" 2>/dev/null
  if [ "$status" -eq 0 ] && [ "$acceptance_complete" -eq 1 ]; then
    final_status="PASS"
    record_result "overall" pass "PASS" "PASS"
  else
    record_result "overall" fail "PASS" "FAIL" "exit_status=$status"
    if [ "$status" -eq 0 ]; then status=1; fi
  fi
  write_acceptance_summary "$FINAL_SUMMARY" "$final_status"
  printf 'validity_tsv\tlabel=%s\tvalid=%s\tinvalid_reason=%s\tresults=%s\treport=%s\n' \
    "$LABEL" "$([ "$final_status" = PASS ] && echo 1 || echo 0)" "$([ "$final_status" = PASS ] && echo ok || echo acceptance-failed)" \
    "$RESULTS_TSV" "$REPORT_LOG" >>"$RUN_CONTEXT"
  exit "$status"
}
trap acceptance_finish EXIT
exec > >(tee -a "$REPORT_LOG") 2>&1

binary_path="$ROOT/magik-gui/target/armv7-unknown-linux-gnueabihf/release-device/mister-magik-fb"
deployed_sha256="$(bench_context_remote_sha256 "$MISTER" "$REMOTE_BIN" || true)"
deployed_sha256="${deployed_sha256:-missing}"
if ! bench_context_require_binary_contract "$binary_path" "$deployed_sha256" ui release-device launcher; then
  echo "catalog acceptance binary identity verification failed local=$(bench_context_sha256_file "$binary_path") deployed=$deployed_sha256 features=$(bench_context_binary_features "$binary_path") expected_features=ui" >&2
  exit 1
fi
binary_fields="$(bench_context_binary_fields release-device launcher ui "$binary_path" production verified "$deployed_sha256")"
source_fields="$(bench_context_source_fields "$ROOT")"
printf 'run_context_tsv\tlabel=%s\tcommand=device-catalog-acceptance\t%s\t%s\n' "$LABEL" "$source_fields" "$binary_fields" >"$RUN_CONTEXT"

echo "==> Waiting ${SETTLE_SECS}s for startup refreshes to settle"
sleep "$SETTLE_SECS"

launcher_count="$(
  remote "ps w | grep '[m]ister-magik-fb ui launcher' | wc -l" | last_number
)"
assert_eq "launcher process count" "1" "$launcher_count"

refresh_count="$(
  remote "ps w | grep '[m]ister-magik-fb library-refresh' | wc -l" | last_number
)"
assert_eq "active library-refresh count" "0" "$refresh_count"

launcher_pid="$(
  remote "ps w | awk '/[m]ister-magik-fb ui launcher/ { print \$1; exit }'" | last_number
)"
assert_gt_zero "launcher pid" "$launcher_pid"
running_sha256="$(bench_context_remote_sha256 "$MISTER" "/proc/$launcher_pid/exe" || true)"
running_sha256="${running_sha256:-missing}"
local_sha256="$(bench_context_sha256_file "$binary_path")"
if ! acceptance_running_binary_matches "$local_sha256" "$deployed_sha256" "$running_sha256"; then
  record_result "running launcher binary identity" fail "$local_sha256" "$running_sha256" "deployed_sha256=$deployed_sha256 pid=$launcher_pid"
  echo "FAIL: running launcher inode does not match the verified local/deployed binary local=$local_sha256 deployed=$deployed_sha256 running=$running_sha256 pid=$launcher_pid" >&2
  exit 1
fi
record_result "running launcher binary identity" pass "$local_sha256" "$running_sha256" "deployed_sha256=$deployed_sha256 pid=$launcher_pid"
printf 'runtime_binary_tsv\tlabel=%s\tpid=%s\trunning_sha256=%s\tdeployed_sha256=%s\tlocal_sha256=%s\tvalid=1\n' \
  "$LABEL" "$launcher_pid" "$running_sha256" "$deployed_sha256" "$local_sha256" >>"$RUN_CONTEXT"

assert_remote_nonempty "$REMOTE_DB"

catalog_games="$(db_scalar "SELECT count(*) FROM games;")"
catalog_game_rows="$(db_scalar "SELECT count(*) FROM game_rows;")"
catalog_launcher_rows="$(db_scalar "SELECT (SELECT count(*) FROM ui_arcade_preferred)+(SELECT count(*) FROM launcher_catalog_rows);")"
catalog_systems="$(db_scalar "SELECT count(*) FROM systems;")"
catalog_discoveries="$(db_scalar "SELECT CAST(value AS INTEGER) FROM meta WHERE key='discoveries';")"
catalog_pc88_rows="$(db_scalar "SELECT count(*) FROM game_rows g JOIN string_values s ON s.string_id=g.system_string_id WHERE s.value='pc88';")"
catalog_db_bytes="$(remote "wc -c < '$REMOTE_DB'" | last_number)"
assert_eq "PC-8801 boot ROM game row count" "0" "$(db_scalar "SELECT count(*) FROM game_rows g JOIN string_values s ON s.string_id=g.system_string_id WHERE s.value='pc88' AND lower(g.title)='boot';")"
assert_gt_zero "durable discovery count" "$catalog_discoveries"
assert_remote_nonempty "$REMOTE_SUMMARY"
"$MISTER" get "$REMOTE_SUMMARY" "$SUMMARY_COPY" >/dev/null
read -r visible_games visible_systems arcade_games sms_games sms_kind gamegear_games gamegear_kind astrocade_games astrocade_kind < <(
  python3 -c 'import json,sys; d=json.load(open(sys.argv[1])); s={x["id"]:x for x in d["systems"]}; print(d["total_game_count"],len(d["systems"]),len(d["hot_games"]),s["sms"]["count"],s["sms"]["platform_kind"],s["gamegear"]["count"],s["gamegear"]["platform_kind"],s["astrocade"]["count"],s["astrocade"]["platform_kind"])' "$SUMMARY_COPY"
)
catalog_facts="$OUT/catalog-facts.json"
catalog_contract_report="$OUT/catalog-contract.tsv"
python3 "$CATALOG_FIXTURE_TOOL" write-facts \
  --summary "$SUMMARY_COPY" --output "$catalog_facts" \
  --games "$catalog_games" --game-rows "$catalog_game_rows" \
  --launcher-rows "$catalog_launcher_rows" --systems "$catalog_systems" \
  --discoveries "$catalog_discoveries" --db-bytes "$catalog_db_bytes" \
  --anchor "pc88_game_rows=$catalog_pc88_rows" \
  --anchor "arcade_visible=$arcade_games" \
  --anchor "sms_games=$sms_games" --anchor "sms_kind=$sms_kind" \
  --anchor "gamegear_games=$gamegear_games" --anchor "gamegear_kind=$gamegear_kind" \
  --anchor "astrocade_games=$astrocade_games" --anchor "astrocade_kind=$astrocade_kind"
if python3 "$CATALOG_FIXTURE_TOOL" validate \
    --contract "$CATALOG_FIXTURE_CONTRACT" --facts "$catalog_facts" | tee "$catalog_contract_report"; then
  record_result "catalog fixture contract" pass "known corpus and exact counts" "matched" "performance budgets recorded only"
else
  record_result "catalog fixture contract" fail "known corpus and exact counts" "mismatch" "see $catalog_contract_report"
  exit 1
fi
assert_eq "games view/game_rows parity" "$catalog_game_rows" "$catalog_games"
assert_eq "summary/navigation launcher-row parity" "$catalog_launcher_rows" "$visible_games"
assert_eq "summary/database system parity" "$catalog_systems" "$visible_systems"
assert_eq "non-core-location classification diagnostics" "0" "$(db_scalar "SELECT count(*) FROM system_classification_diagnostics WHERE rejected_source != 'core-location';")"
for expected_mismatch in sms gamegear astrocade; do
  assert_eq "$expected_mismatch core-location mismatch diagnostic" "1" "$(db_scalar "SELECT count(*) FROM system_classification_diagnostics WHERE system_id='$expected_mismatch' AND rejected_source='core-location' AND rejected_kind='arcade';")"
done
assert_remote_nonempty "$REMOTE_NAVIGATION"
filter_report="$(catalog_filter_report)"
printf '%s\n' "$filter_report"
filter_parity_status="$(
  printf '%s\n' "$filter_report" | awk '
    /^catalog_filter_parity_tsv[[:space:]]/ {
      for (i = 1; i <= NF; i++) if ($i ~ /^status=/) { sub(/^status=/, "", $i); print $i; exit }
    }
  '
)"
navigation_category_count="$(
  printf '%s\n' "$filter_report" | awk '
    /^catalog_filter_summary_tsv[[:space:]]/ && /source=navigation/ {
      for (i = 1; i <= NF; i++) if ($i ~ /^categories=/) { sub(/^categories=/, "", $i); print $i; exit }
    }
  '
)"
assert_eq "catalog filter projection parity" "ok" "$filter_parity_status"
assert_gt "Arcade normalized category count" "2" "$navigation_category_count"

console_pack_count="$(
  remote "find '$REMOTE_ASSETS' -maxdepth 1 -type f \\( -name '*-screenshots.mmlz4b' -o -name '*-screenshots-320x320.mmlz4b' \\) 2>/dev/null | wc -l" | last_number
)"
if [ "$console_pack_count" -gt 0 ]; then
  asset_entry_tables="$(
    db_scalar "SELECT count(*) FROM sqlite_master WHERE type='table' AND name='asset_entries';"
  )"
  assert_eq "runtime-only screenshot asset table count" "0" "$asset_entry_tables"
fi

if [ "$(pack_exists "arcade-screenshots-320x320.mmlz4b")" = "yes" ]; then
  echo "ok: size-qualified arcade screenshot pack installed"
fi

if [ "$(pack_exists ".screenshot-media-state.json")" = "yes" ]; then
  size_state_count="$(
    remote "grep -c 'screenshots-320x320\\.mmlz4b' '$REMOTE_ASSETS/.screenshot-media-state.json' 2>/dev/null || true" | last_number
  )"
  if [ "${size_state_count:-0}" -gt 0 ]; then
    echo "ok: media state size-qualified local_path count = $size_state_count"
    cache_state_count="$(
      remote "grep -c 'cf_cache_status\\|content_length\\|effective_url' '$REMOTE_ASSETS/.screenshot-media-state.json' 2>/dev/null || true" | last_number
    )"
    assert_gt_zero "media state cache metadata count" "$cache_state_count"
  else
    echo "ok: media state present without size-qualified runtime downloads"
  fi
else
  echo "ok: media state not present; runtime downloader has not published packs on this device"
fi

progress_log_count="$(
  remote "grep -h 'screenshot_media_progress' /tmp/mister-magik-slint.log /tmp/mister-magik-launcher.log '$MISTER_MAGIK_APP_DIR'/*.log 2>/dev/null | wc -l" | last_number
)"
if [ "${progress_log_count:-0}" -gt 0 ]; then
  echo "ok: screenshot media progress log rows = $progress_log_count"
else
  echo "ok: screenshot media progress log not captured in known log files"
fi

catalog_seed_count="$(
  remote "grep -h 'screenshot_media_catalog_ensure' /tmp/mister-magik-slint.log /tmp/mister-magik-launcher.log '$MISTER_MAGIK_APP_DIR'/*.log 2>/dev/null | wc -l" | last_number
)"
if [ "${catalog_seed_count:-0}" -gt 0 ]; then
  echo "ok: cached catalog screenshot media ensure rows = $catalog_seed_count"
else
  echo "ok: cached catalog screenshot media ensure rows not captured in known log files"
fi

if [ "$RACE_REFRESH" -eq 1 ]; then
  echo "==> Triggering duplicate library-refresh race"
  race_output="$(
    remote "mkdir -p /tmp/mister-magik; rm -f /tmp/mister-magik/refresh-race-a.log /tmp/mister-magik/refresh-race-b.log; '$REMOTE_BIN' library-refresh >/tmp/mister-magik/refresh-race-a.log 2>&1 & first=\$!; sleep 0.3; '$REMOTE_BIN' library-refresh >/tmp/mister-magik/refresh-race-b.log 2>&1; second_status=\$?; echo second_status=\$second_status; cat /tmp/mister-magik/refresh-race-b.log; wait \$first"
  )"
  echo "$race_output"
  if ! printf '%s\n' "$race_output" | grep -q 'library_refresh[[:space:]]skipped[[:space:]]active_pid='; then
    record_result "duplicate refresh single-flight" fail "skip" "not-skipped"
    echo "FAIL: duplicate refresh did not report single-flight skip" >&2
    exit 1
  fi
  record_result "duplicate refresh single-flight" pass "skip" "skipped"
  echo "ok: duplicate refresh skipped via single-flight"
fi

acceptance_complete=1
echo "device catalog acceptance: ok"
