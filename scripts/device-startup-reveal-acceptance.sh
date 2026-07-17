#!/usr/bin/env bash
# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

# Device acceptance for the state-based launcher startup reveal flow.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
MISTER="$ROOT/scripts/mister"
LABEL="${1:-startup-reveal-$(date -u +%Y%m%dT%H%M%SZ)}"
MODE="${MISTER_STARTUP_REVEAL_MODE:-all}"
REMOTE_DB="/media/fat/mister-magik-dev/library.sqlite3"
REMOTE_SUMMARY="/media/fat/mister-magik-dev/library.summary.json"
REMOTE_ENV="/media/fat/mister-magik-dev/launcher.env"
REMOTE_LOG="/tmp/mister-magik-slint.log"
RETURN_STATE="/tmp/mister-magik/launcher-return-state.json"
TSV="$ROOT/history/toolchain-bench/results-startup-reveal.tsv"
OUT="$ROOT/build/startup-reveal-acceptance/$LABEL"
SELECTED="${MISTER_STARTUP_REVEAL_SELECTED_INDEX:-17}"
[[ "$SELECTED" =~ ^[0-9]+$ ]] || {
  echo "MISTER_STARTUP_REVEAL_SELECTED_INDEX must be a non-negative integer" >&2
  exit 2
}
case "$MODE" in
  all|cold|warm|return) ;;
  *) echo "MISTER_STARTUP_REVEAL_MODE must be all, cold, warm, or return" >&2; exit 2 ;;
esac

COLD_BACKUP_DB="/media/fat/mister-magik-dev/library.sqlite3.startup-reveal-$LABEL.bak"
COLD_BACKUP_SUMMARY="/media/fat/mister-magik-dev/library.summary.startup-reveal-$LABEL.bak"

cleanup() {
  remote "rm -f '$REMOTE_ENV'; if [ -f '$COLD_BACKUP_DB' ]; then mv '$COLD_BACKUP_DB' '$REMOTE_DB'; fi; if [ -f '$COLD_BACKUP_SUMMARY' ]; then mv '$COLD_BACKUP_SUMMARY' '$REMOTE_SUMMARY'; fi; if [ -s '$RETURN_STATE' ] && [ -p /dev/MiSTer_cmd ]; then printf 'load_core menu.rbf\n' > /dev/MiSTer_cmd; fi; sync" >/dev/null 2>&1 || true
}
trap cleanup EXIT

mkdir -p "$(dirname "$TSV")" "$OUT"
if [ ! -f "$TSV" ]; then
  printf 'label\tmode\titeration\treveal_ms\tinput_enabled_ms\tcatalog_ready_ms\tfirst_frame_ms\tpreview_state\tresult\tnotes\n' >"$TSV"
fi

remote() {
  "$MISTER" run "$1"
}

record() {
  printf '%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\n' \
    "$LABEL" "$1" "$2" "$3" "$4" "$5" "$6" "$7" "$8" "$9" >>"$TSV"
}

fail() {
  echo "FAIL: $*" >&2
  exit 1
}

reset_remote_logs() {
  remote "rm -f '$REMOTE_LOG' /tmp/mister-magik/events.jsonl; sync" >/dev/null
}

wait_status() {
  local label="$1" timeout="$2" expr="$3" elapsed=0 tmp
  tmp="$OUT/status-${label//[^A-Za-z0-9_.-]/_}.json"
  echo "==> $label"
  while [ "$elapsed" -le "$timeout" ]; do
    if "$MISTER" status --json >"$tmp" 2>"$tmp.err" &&
       python3 - "$tmp" "$expr" <<'PY'
import json, sys
with open(sys.argv[1], encoding="utf-8") as f:
    data = json.load(f)
runtime = data.get("runtime") or {}
runtime["main_status"] = runtime.get("main_status") or {}
runtime["slint_status"] = runtime.get("slint_status") or {}
data["runtime"] = runtime
ok = bool(eval(sys.argv[2], {"__builtins__": {"int": int, "str": str, "len": len}}, {"data": data}))
raise SystemExit(0 if ok else 1)
PY
    then
      echo "ok: $label"
      return 0
    fi
    sleep 2
    elapsed=$((elapsed + 2))
  done
  "$MISTER" status || true
  return 1
}

pull_log() {
  local name="$1"
  "$MISTER" get "$REMOTE_LOG" "$OUT/$name.log" >/dev/null || true
  [ -s "$OUT/$name.log" ] || fail "missing launcher log for $name"
  printf '%s\n' "$OUT/$name.log"
}

metric_ms() {
  local log="$1" event="$2"
  awk -F '\t' -v event="$event" '$1 == "startup_timing" && $2 == event { ms=$3; sub(/ms$/, "", ms); print ms; exit }' "$log"
}

detail_field() {
  local log="$1" event="$2" key="$3"
  awk -F '\t' -v event="$event" -v key="$key" '
    $1 == "startup_timing" && $2 == event {
      split($4, parts, " ")
      for (i in parts) {
        split(parts[i], kv, "=")
        if (kv[1] == key) { print kv[2]; exit }
      }
    }
  ' "$log"
}

require_event() {
  local log="$1" event="$2"
  grep -q $'^startup_timing\t'"$event"$'\t' "$log" || fail "$event missing in $log"
}

assert_mode() {
  local log="$1" mode="$2"
  grep -q $'^startup_timing\tstartup_entry_classified\t.*mode='"$mode" "$log" ||
    fail "expected startup mode $mode in $log"
}

collect_and_assert_common() {
  local mode="$1" iteration="$2" log="$3"
  require_event "$log" launcher_revealed
  require_event "$log" launcher_input_enabled
  local reveal input first ready preview
  reveal="$(metric_ms "$log" launcher_revealed)"
  input="$(metric_ms "$log" launcher_input_enabled)"
  first="$(metric_ms "$log" first_frame)"
  ready="$(metric_ms "$log" library_ready)"
  preview="$(detail_field "$log" launcher_reveal_ready preview_state)"
  [ -n "$reveal" ] || fail "missing reveal_ms for $mode"
  [ -n "$input" ] || fail "missing input_enabled_ms for $mode"
  [ "$input" -ge "$reveal" ] || fail "$mode input_enabled_ms $input < reveal_ms $reveal"
  record "$mode" "$iteration" "$reveal" "$input" "${ready:-}" "${first:-}" "${preview:-}" pass "-"
}

run_cold() {
  echo "== cold no-catalog reveal"
  remote "if [ -f '$REMOTE_DB' ]; then cp '$REMOTE_DB' '$COLD_BACKUP_DB'; fi; if [ -f '$REMOTE_SUMMARY' ]; then cp '$REMOTE_SUMMARY' '$COLD_BACKUP_SUMMARY'; fi; rm -f '$REMOTE_DB' '$REMOTE_SUMMARY' '$REMOTE_ENV' '$RETURN_STATE' '$REMOTE_LOG' /tmp/mister-magik/events.jsonl; sync"
  "$MISTER" reboot-wait --direct-reset
  wait_status cold-ready 300 "data['runtime']['slint_status'].get('input_enabled') is True and data['runtime']['slint_status'].get('catalog_ready') is True" ||
    fail "cold startup did not reach input_enabled"
  local log
  log="$(pull_log cold)"
  assert_mode "$log" cold_no_catalog
  require_event "$log" startup_splash_visible
  require_event "$log" startup_splash_done
  require_event "$log" catalog_progress_revealed
  require_event "$log" library_ready
  local splash_done ready reveal
  splash_done="$(metric_ms "$log" startup_splash_done)"
  ready="$(metric_ms "$log" library_ready)"
  reveal="$(metric_ms "$log" launcher_revealed)"
  [ "$splash_done" -ge 1900 ] || fail "cold splash too short: ${splash_done}ms"
  [ "$reveal" -ge "$ready" ] || fail "cold reveal $reveal before library_ready $ready"
  collect_and_assert_common cold_no_catalog 1 "$log"
  remote "if [ -f '$COLD_BACKUP_DB' ]; then mv '$COLD_BACKUP_DB' '$REMOTE_DB'; fi; if [ -f '$COLD_BACKUP_SUMMARY' ]; then mv '$COLD_BACKUP_SUMMARY' '$REMOTE_SUMMARY'; fi; sync"
  remote "printf 'mister_magik_restart_launcher\n' > /dev/MiSTer_cmd" >/dev/null || true
}

run_warm() {
  echo "== warm catalog reveal"
  remote "test -s '$REMOTE_DB'" >/dev/null || fail "warm scenario needs an existing $REMOTE_DB"
  local i log
  for i in 1 2 3 4 5; do
    reset_remote_logs
    "$MISTER" reboot-wait
    wait_status "warm-$i" 90 "data['runtime']['slint_status'].get('input_enabled') is True and data['runtime']['slint_status'].get('catalog_ready') is True" ||
      fail "warm iteration $i did not reach input_enabled"
    log="$(pull_log "warm-$i")"
    assert_mode "$log" warm_catalog
    if grep -q $'^startup_timing\tstartup_splash_visible\t' "$log"; then
      fail "warm iteration $i showed splash"
    fi
    collect_and_assert_common warm_catalog "$i" "$log"
  done
}

run_return() {
  echo "== return-from-game reveal"
  local env_file
  env_file="$(mktemp)"
  trap 'rm -f "$env_file"' RETURN
  {
    printf 'export MISTER_CATALOG_REFRESH=off\n'
    printf 'export MISTER_LAUNCHER_START_SCREEN=arcade\n'
    printf 'export MISTER_LAUNCHER_START_SYSTEM=arcade\n'
    printf 'export MISTER_ARCADE_SELECTED_INDEX=%s\n' "$SELECTED"
    printf 'export MISTER_LAUNCHER_AUTO_LAUNCH_SELECTED=1\n'
  } >"$env_file"

  local i log old_pid
  for i in 1 2 3; do
    old_pid="$(remote "pidof mister-magik-fb 2>/dev/null || true")"
    old_pid="${old_pid##* }"
    old_pid="${old_pid:-0}"
    remote "rm -f '$REMOTE_ENV' '$RETURN_STATE' '$REMOTE_LOG' /tmp/mister-magik/events.jsonl; printf 'mister_magik_restart_launcher\n' > /dev/MiSTer_cmd"
    wait_status "return-prime-$i" 60 "data['runtime']['main_status'].get('launcher_state') == 'LauncherActive' and int(data['runtime']['main_status'].get('launcher_pid') or 0) != $old_pid" ||
      fail "return iteration $i could not prime launcher"
    "$MISTER" put "$env_file" "$REMOTE_ENV" >/dev/null
    remote "printf 'mister_magik_restart_launcher\n' > /dev/MiSTer_cmd"
    wait_status "return-state-written-$i" 90 "data['runtime']['main_status'].get('launcher_state') in ('HandoffToGame', 'Unconfigured') or data['runtime']['main_status'].get('launcher_active') is False" ||
      true
    remote "test -s '$RETURN_STATE'" >/dev/null || fail "return state not written for iteration $i"
    remote "grep -Eq '\"game_index\"[[:space:]]*:[[:space:]]*$SELECTED([,}])' '$RETURN_STATE'" >/dev/null ||
      fail "return iteration $i captured the wrong arcade row; expected $SELECTED"
    reset_remote_logs
    remote "rm -f '$REMOTE_ENV'; printf 'load_core menu.rbf\n' > /dev/MiSTer_cmd"
    wait_status "return-restored-$i" 90 "data['runtime']['slint_status'].get('input_enabled') is True and data['runtime']['slint_status'].get('screen') == 'arcade' and int(data['runtime']['slint_status'].get('arcade_selected', -1)) == $SELECTED" ||
      fail "return iteration $i did not restore arcade row $SELECTED"
    remote "test ! -e '$RETURN_STATE'" >/dev/null || fail "return state not consumed for iteration $i"
    log="$(pull_log "return-$i")"
    assert_mode "$log" return_from_game
    require_event "$log" return_context_restored
    require_event "$log" return_preview_ready
    collect_and_assert_common return_from_game "$i" "$log"
  done
}

case "$MODE" in
  all) run_cold; run_warm; run_return ;;
  cold) run_cold ;;
  warm) run_warm ;;
  return) run_return ;;
esac

echo "startup reveal acceptance passed; rows appended to $TSV"
