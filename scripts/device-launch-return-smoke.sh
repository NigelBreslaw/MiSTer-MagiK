#!/usr/bin/env bash
# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

# Device smoke for launcher return state.
#
# Drives a real launcher-selected game handoff without controller injection:
#   1. start Arcade at two forced selected rows
#   2. use a gated launcher env hook to launch that selected game
#   3. remove launcher.env
#   4. return through the agent's acknowledged Main operation
#   5. assert MagiK consumes /tmp return state and restores Arcade at that row
#   6. report total return, black hold, and input-ready timing for both games
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
MISTER="$HERE/scripts/mister"
source "$HERE/scripts/lib/magik-layout.sh"
source "$HERE/scripts/lib/latch-readiness-lib.sh"
source "$HERE/scripts/lib/diagnostic-output-lib.sh"
magik_layout_select dev
REMOTE_ENV="/media/fat/mister-magik-dev/launcher.env"
RETURN_STATE="/tmp/mister-magik/launcher-return-state.json"
STALE_RETURN_STATE="/tmp/mister-magik/stale-launcher-return-state.json"
STATUS="/tmp/mister-magik/status.json"
MAIN_STATUS="/tmp/mister-magik/main-status.json"
REMOTE_LOG="/tmp/mister-magik-slint.log"
REMOTE_EVENTS="/tmp/mister-magik/events.jsonl"
RETURN_START_MS="/tmp/mister-magik/launcher-return-start-ms"
REMOTE_BIN="/media/fat/mister-magik-dev/mister-magik-fb"
GAME_SETTLE_SECS="${GAME_SETTLE_SECS:-8}"
MAX_TOTAL_RETURN_MS="${MAX_TOTAL_RETURN_MS:-3000}"
MAX_BLACK_MS="${MAX_BLACK_MS:-2000}"

extract_return_metrics() {
  local input="$1" line match="" count=0
  local pattern=$'^[0-9]+\t[0-9]+\t[0-9]+\t[0-9]+\t[0-9]+\t[0-9]+\t[0-9]+\t[0-9]+$'
  while IFS= read -r line; do
    if [[ "$line" =~ $pattern ]]; then
      match="$line"
      count=$((count + 1))
    fi
  done <<<"$input"
  if [[ "$count" -ne 1 ]]; then
    return 1
  fi
  printf '%s\n' "$match"
}

if [[ "${1:-}" == "--self-test" ]]; then
  [[ "$(extract_return_metrics $'return_wait_heartbeat\n100\t178\t556\t565\t465\t378\t78\t9')" == $'100\t178\t556\t565\t465\t378\t78\t9' ]]
  ! extract_return_metrics "return_wait_heartbeat" >/dev/null
  ! extract_return_metrics $'1\t2\t3\t4\t5\t6\t7\t8\n9\t10\t11\t12\t13\t14\t15\t16' >/dev/null
  ! extract_return_metrics $'return_wait_heartbeat\t\t\t' >/dev/null
  echo "device-launch-return-smoke self-test ok"
  exit 0
fi

ARTIFACTS_DIR=""
LABEL="device-launch-return-smoke"
while [ "$#" -gt 0 ]; do
  case "$1" in
    --artifacts-dir)
      [ "$#" -ge 2 ] || {
        echo "--artifacts-dir requires a path" >&2
        exit 2
      }
      ARTIFACTS_DIR="$2"
      shift 2
      ;;
    --label)
      [ "$#" -ge 2 ] || {
        echo "--label requires a value" >&2
        exit 2
      }
      LABEL="$2"
      shift 2
      ;;
    --)
      shift
      break
      ;;
    -*)
      echo "unknown option: $1" >&2
      exit 2
      ;;
    *)
      break
      ;;
  esac
done

if [ "$#" -eq 0 ]; then
  GAME_ROWS=(17 42)
elif [ "$#" -eq 2 ]; then
  GAME_ROWS=("$1" "$2")
else
  echo "usage: scripts/device-launch-return-smoke.sh [--artifacts-dir DIR] [--label LABEL] [first-index second-index]" >&2
  exit 2
fi

for row in "${GAME_ROWS[@]}"; do
  if [[ ! "$row" =~ ^[0-9]+$ ]]; then
    echo "selected indexes must be non-negative integers" >&2
    exit 2
  fi
done

if [ -z "$ARTIFACTS_DIR" ]; then
  ARTIFACTS_DIR="$HERE/build/launch-return/device-launch-return-smoke-$(date -u +%Y%m%dT%H%M%SZ)-$$"
elif [[ "$ARTIFACTS_DIR" != /* ]]; then
  ARTIFACTS_DIR="$HERE/$ARTIFACTS_DIR"
fi
mkdir -p "$ARTIFACTS_DIR"
exec 3>&1 4>&2
exec > >(tee "$ARTIFACTS_DIR/run.log") 2>&1
TEE_PID=$!
printf 'launch_return_artifacts_tsv\tpath=%s\n' "$ARTIFACTS_DIR"
deployed_sha256="$("$MISTER" run "sha256sum '$REMOTE_BIN'" | awk 'NR == 1 { print $1 }')"
[[ "$deployed_sha256" =~ ^[0-9a-f]{64}$ ]] || {
  echo "invalid deployed binary SHA-256: $deployed_sha256" >&2
  exit 1
}
printf 'schema\tlabel\tcommit\tdirty\tcommand\tmax_total_return_ms\tmax_black_ms\tdeployed_binary_path\tdeployed_sha256\tstarted_utc\n' >"$ARTIFACTS_DIR/run-context.tsv"
printf '1\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\n' \
  "$LABEL" \
  "$(git -C "$HERE" rev-parse HEAD)" \
  "$(git -C "$HERE" status --short | wc -l | tr -d ' ')" \
  "$0 $*" \
  "$MAX_TOTAL_RETURN_MS" \
  "$MAX_BLACK_MS" \
  "$REMOTE_BIN" \
  "$deployed_sha256" \
  "$(date -u +%Y-%m-%dT%H:%M:%SZ)" >>"$ARTIFACTS_DIR/run-context.tsv"
printf 'schema\tlabel\titeration\trow\tgame_path\tgame_title\treturn_start_ms\tblack_start_ms\treveal_ms\tinput_ms\ttotal_return_ms\tblack_ms\tlaunch_to_black_ms\treveal_to_input_ms\ttotal_budget_ms\tblack_budget_ms\tresult\n' >"$ARTIFACTS_DIR/report.tsv"

remote() {
  "$MISTER" run "$1"
}

send_main_command() {
  local command="$1"
  case "$command" in
    "load_core menu.rbf") "$MISTER" agent magik return-to-launcher ;;
    "mister_magik_restart_launcher") "$MISTER" agent magik restart-launcher ;;
    *) echo "unsupported acknowledged Main command: $command" >&2; return 2 ;;
  esac
}

send_timed_main_command() {
  local command="$1"
  remote "awk '{printf \"%.0f\\n\", \$1 * 1000}' /proc/uptime > '$RETURN_START_MS'"
  send_main_command "$command"
}

launcher_is_active() {
  remote "grep -q '\"launcher_state\":\"LauncherActive\"' '$MAIN_STATUS' 2>/dev/null && grep -q '\"scene\":\"launcher\"' '$STATUS' 2>/dev/null" >/dev/null 2>&1
}

dump_failure_artifacts() {
  local prefix="$ARTIFACTS_DIR/failure"
  capture_remote_file "$MAIN_STATUS" "$prefix-main-status.json"
  capture_remote_file "$STATUS" "$prefix-status.json"
  capture_remote_file "$RETURN_STATE" "$prefix-return-state.json"
  capture_remote_file "$REMOTE_LOG" "$prefix-launcher.log"
  capture_remote_file "$REMOTE_EVENTS" "$prefix-events.jsonl"
  remote "cat /sys/module/MiSTer_fb/parameters/mode 2>/dev/null || true" >"$prefix-fb-mode.txt" || true
  remote "ps w | grep -E 'MiSTer|MiSTer_MagiKDev|mister-magik-fb' | grep -v grep || true" >"$prefix-processes.txt" || true
  diagnostic_failure_summary "launch-return smoke" "$ARTIFACTS_DIR" \
    "$prefix-main-status.json" "$prefix-launcher.log"
}

capture_remote_file() {
  local remote_path="$1" local_path="$2"
  "$MISTER" get "$remote_path" "$local_path" >/dev/null 2>&1 || true
}

capture_iteration_artifacts() {
  local iteration="$1"
  capture_remote_file "$STATUS" "$ARTIFACTS_DIR/iteration-$iteration-status.json"
  capture_remote_file "$MAIN_STATUS" "$ARTIFACTS_DIR/iteration-$iteration-main-status.json"
  capture_remote_file "$REMOTE_LOG" "$ARTIFACTS_DIR/iteration-$iteration-launcher.log"
  capture_remote_file "$REMOTE_EVENTS" "$ARTIFACTS_DIR/iteration-$iteration-events.jsonl"
  capture_remote_file "$STALE_RETURN_STATE" "$ARTIFACTS_DIR/iteration-$iteration-return-state.json"
  capture_remote_file "$RETURN_START_MS" "$ARTIFACTS_DIR/iteration-$iteration-return-start-ms.txt"
  remote "cat /sys/module/MiSTer_fb/parameters/mode 2>/dev/null || true" \
    >"$ARTIFACTS_DIR/iteration-$iteration-fb-mode.txt" || true
}

wait_remote() {
  local label="$1" timeout="$2" expr="$3"
  if remote "elapsed=0; while [ \"\$elapsed\" -lt '$timeout' ]; do if $expr; then exit 0; fi; echo wait_remote_heartbeat >&2; sleep 1; elapsed=\$((elapsed + 1)); done; exit 1" >/dev/null 2>&1; then
    echo "ok: $label"
    return 0
  fi
  echo "FAIL: $label" >&2
  dump_failure_artifacts
  return 1
}

prove_latch_ready() {
  local label="$1" artifact="$2" output status
  set +e
  output="$(latch_readiness_probe "$MISTER" 2>&1)"
  status="$?"
  set -e
  printf '%s\n' "$output" | tee "$artifact"
  if [[ "$status" -eq 0 ]]; then
    echo "ok: latch ready $label"
    return 0
  fi
  if ! latch_readiness_is_contract_failure "$output"; then
    echo "FAIL: latch readiness transport failed $label; stopping device calls" >&2
    return "$status"
  fi
  echo "FAIL: latch unavailable $label" >&2
  dump_failure_artifacts
  return 1
}

tmp_env="$(mktemp)"
tmp_results="$(mktemp)"
cleanup() {
  rm -f "$tmp_env" "$tmp_results"
  remote "rm -f '$REMOTE_ENV' '$RETURN_START_MS'" >/dev/null 2>&1 || true
}
trap cleanup EXIT

echo "==> Resetting launcher state"
remote "rm -f '$REMOTE_ENV' '$RETURN_STATE' '$STALE_RETURN_STATE' '$REMOTE_LOG' '$REMOTE_EVENTS'"
if ! launcher_is_active; then
  send_main_command "mister_magik_restart_launcher"
fi
wait_remote "launcher active before smoke" 60 "grep -q '\"launcher_state\":\"LauncherActive\"' '$MAIN_STATUS' 2>/dev/null && grep -q '\"scene\":\"launcher\"' '$STATUS' 2>/dev/null"

for iteration in 1 2; do
  selected="${GAME_ROWS[$((iteration - 1))]}"
  cat >"$tmp_env" <<EOF
export MISTER_CATALOG_REFRESH=off
export MISTER_LAUNCHER_START_SCREEN=arcade
export MISTER_LAUNCHER_START_SYSTEM=arcade
export MISTER_ARCADE_SELECTED_INDEX=$selected
export MISTER_LAUNCHER_AUTO_LAUNCH_SELECTED=1
EOF

  echo "==> [$iteration/2] Starting Arcade at selected row $selected and auto-launching"
  remote "rm -f '$REMOTE_ENV' '$RETURN_STATE' '$REMOTE_LOG' '$REMOTE_EVENTS' '$RETURN_START_MS'"
  "$MISTER" put "$tmp_env" "$REMOTE_ENV" >/dev/null
  send_main_command "mister_magik_restart_launcher"
  wait_remote "game $iteration launch return state written" 60 "test -s '$RETURN_STATE'"
  wait_remote "game $iteration return state recorded row $selected" 5 "grep -q '\"game_index\": $selected' '$RETURN_STATE' 2>/dev/null || grep -q '\"game_index\":$selected' '$RETURN_STATE' 2>/dev/null"
  remote "cp '$RETURN_STATE' '$STALE_RETURN_STATE'"
  game_path="$(remote "sed -n 's/.*\"game_path\": \"\(.*\)\",/\1/p' '$RETURN_STATE'")"
  game_title="${game_path##*/}"
  game_title="${game_title%.mra}"
  echo "==> [$iteration/2] Waiting ${GAME_SETTLE_SECS}s for $game_title to settle"
  sleep "$GAME_SETTLE_SECS"

  echo "==> [$iteration/2] Returning from $game_title via load_core menu.rbf"
  remote "rm -f '$REMOTE_ENV' '$REMOTE_LOG' '$REMOTE_EVENTS'"
  if ! send_timed_main_command "load_core menu.rbf"; then
    echo "FAIL: acknowledged return-to-launcher failed" >&2
    remote "rm -f '$REMOTE_ENV' '$RETURN_STATE'"
    exit 1
  fi

  metrics_raw="$(remote "elapsed=0; while [ \"\$elapsed\" -lt 900 ]; do expected=\$(sed -n 's/.*\"game_path\": \"\(.*\)\",/\1/p' '$STALE_RETURN_STATE'); black_start_ms=\$(sed -n '/\"event\":\"launcher_spawn_black_route_start\"/s/.*\"ts_boot_ms\":\([0-9][0-9]*\).*/\1/p' '$REMOTE_EVENTS' | tail -1); reveal_boot_ms=\$(sed -n '/\"event\":\"launcher_revealed\"/s/.*\"ts_boot_ms\":\([0-9][0-9]*\).*/\1/p' '$REMOTE_EVENTS' | tail -1); input_boot_ms=\$(sed -n '/\"event\":\"launcher_input_enabled\"/s/.*\"ts_boot_ms\":\([0-9][0-9]*\).*/\1/p' '$REMOTE_EVENTS' | tail -1); if test -n \"\$expected\" && test -n \"\$black_start_ms\" && test -n \"\$reveal_boot_ms\" && test -n \"\$input_boot_ms\" && grep -q '\"startup_mode\":\"return_from_game\"' '$STATUS' 2>/dev/null && grep -q '\"screen\":\"arcade\"' '$STATUS' 2>/dev/null && grep -q '\"arcade_selected\":$selected' '$STATUS' 2>/dev/null && grep -q '\"input_enabled\":true' '$STATUS' 2>/dev/null && grep -Fq \"system_id=arcade filter=all game_path=\$expected game_index=$selected\" '$REMOTE_LOG' 2>/dev/null && test ! -e '$RETURN_STATE'; then start_ms=\$(cat '$RETURN_START_MS'); printf '%s\\t%s\\t%s\\t%s\\t%s\\t%s\\t%s\\t%s\\n' \"\$start_ms\" \"\$black_start_ms\" \"\$reveal_boot_ms\" \"\$input_boot_ms\" \"\$((input_boot_ms - start_ms))\" \"\$((reveal_boot_ms - black_start_ms))\" \"\$((black_start_ms - start_ms))\" \"\$((input_boot_ms - reveal_boot_ms))\"; exit 0; fi; if [ \"\$((elapsed % 50))\" -eq 0 ]; then echo return_wait_heartbeat >&2; fi; sleep 0.1; elapsed=\$((elapsed + 1)); done; exit 1")" || {
    echo "FAIL: game $iteration did not restore an interactive Arcade context" >&2
    dump_failure_artifacts
    exit 1
  }
  metrics="$(extract_return_metrics "$metrics_raw")" || {
    echo "FAIL: game $iteration returned invalid timing output" >&2
    printf '%s\n' "$metrics_raw" >&2
    dump_failure_artifacts
    exit 1
  }
  prove_latch_ready "after game $iteration return" "$ARTIFACTS_DIR/iteration-$iteration-latch-readiness.tsv"
  IFS=$'\t' read -r return_start_ms black_start_ms reveal_ms input_ms total_return_ms black_ms launch_to_black_ms reveal_to_input_ms <<<"$metrics"
  capture_iteration_artifacts "$iteration"
  result="pass"
  if [ "$total_return_ms" -gt "$MAX_TOTAL_RETURN_MS" ] || [ "$black_ms" -gt "$MAX_BLACK_MS" ]; then
    result="fail"
  fi
  printf '1\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\n' \
    "$LABEL" "$iteration" "$selected" "$game_path" "$game_title" \
    "$return_start_ms" "$black_start_ms" "$reveal_ms" "$input_ms" \
    "$total_return_ms" "$black_ms" "$launch_to_black_ms" "$reveal_to_input_ms" \
    "$MAX_TOTAL_RETURN_MS" "$MAX_BLACK_MS" "$result" >>"$ARTIFACTS_DIR/report.tsv"
  if [ "$total_return_ms" -gt "$MAX_TOTAL_RETURN_MS" ] || [ "$black_ms" -gt "$MAX_BLACK_MS" ]; then
    echo "FAIL: game $iteration return exceeded gate: total=${total_return_ms}ms/${MAX_TOTAL_RETURN_MS}ms black=${black_ms}ms/${MAX_BLACK_MS}ms" >&2
    dump_failure_artifacts
    exit 1
  fi
  printf '%s\t%s\t%s\t%s\t%s\t%s\t%s\n' "$iteration" "$selected" "$game_title" "$total_return_ms" "$black_ms" "$launch_to_black_ms" "$reveal_to_input_ms" >>"$tmp_results"
  printf 'return_timing_tsv\titeration=%s\trow=%s\tgame=%s\ttotal_return_ms=%s\tblack_ms=%s\tlaunch_to_black_ms=%s\treveal_to_input_ms=%s\n' "$iteration" "$selected" "$game_title" "$total_return_ms" "$black_ms" "$launch_to_black_ms" "$reveal_to_input_ms"
done

echo "==> Verifying stale return state is ignored without volatile return flag"
cat >"$tmp_env" <<EOF
export MISTER_MAGIK_RETURN_TO_LAUNCHER=0
EOF
"$MISTER" put "$tmp_env" "$REMOTE_ENV" >/dev/null
remote "cp '$STALE_RETURN_STATE' '$RETURN_STATE'"
send_main_command "mister_magik_restart_launcher"
wait_remote "stale return state ignored on normal launcher start" 60 "grep -q '\"screen\":\"home\"' '$STATUS' 2>/dev/null && grep -q '\"start_screen\":\"home\"' '$STATUS' 2>/dev/null"
wait_remote "stale return state consumed" 10 "test ! -e '$RETURN_STATE'"
prove_latch_ready "after normal launcher start" "$ARTIFACTS_DIR/normal-start-latch-readiness.tsv"
capture_remote_file "$STATUS" "$ARTIFACTS_DIR/normal-start-status.json"
capture_remote_file "$MAIN_STATUS" "$ARTIFACTS_DIR/normal-start-main-status.json"
capture_remote_file "$REMOTE_LOG" "$ARTIFACTS_DIR/normal-start-launcher.log"
capture_remote_file "$REMOTE_EVENTS" "$ARTIFACTS_DIR/normal-start-events.jsonl"
printf 'stale_state_result_tsv\tlabel=%s\tignored=1\tconsumed=1\n' "$LABEL" \
  >"$ARTIFACTS_DIR/stale-state-result.tsv"
remote "rm -f '$REMOTE_ENV' '$STALE_RETURN_STATE'"

echo
printf '%-4s %-6s %-36s %16s %10s %18s %18s\n' "Run" "Row" "Game" "Total return ms" "Black ms" "Launch-black ms" "Reveal-input ms"
printf '%-4s %-6s %-36s %16s %10s %18s %18s\n' "----" "------" "------------------------------------" "----------------" "----------" "------------------" "------------------"
while IFS=$'\t' read -r iteration selected game_title total_return_ms black_ms launch_to_black_ms reveal_to_input_ms; do
  printf '%-4s %-6s %-36s %16s %10s %18s %18s\n' "$iteration" "$selected" "$game_title" "$total_return_ms" "$black_ms" "$launch_to_black_ms" "$reveal_to_input_ms"
done <"$tmp_results"

echo "==> Device launch return smoke passed"
exec 1>&3 2>&4
wait "$TEE_PID"
(
  cd "$ARTIFACTS_DIR"
  find . -type f ! -name manifest.sha256 -print0 | sort -z | xargs -0 shasum -a 256
) >"$ARTIFACTS_DIR/manifest.sha256"
