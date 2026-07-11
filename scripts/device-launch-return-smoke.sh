#!/usr/bin/env bash
# Device smoke for launcher return state.
#
# Drives a real launcher-selected game handoff without controller injection:
#   1. start Arcade at a forced selected row
#   2. use a gated launcher env hook to launch that selected game
#   3. remove launcher.env
#   4. return from the running core with Main's active-core command when the
#      FIFO has a reader, or the release-acceptance raw reboot recovery path
#      when the active core has replaced Main without a FIFO reader
#   5. assert MagiK consumes /tmp return state and restores Arcade at that row
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
MISTER="$HERE/scripts/mister"
source "$HERE/scripts/mister-fifo-lib.sh"
REMOTE_ENV="/media/fat/mister-magik/launcher.env"
RETURN_STATE="/tmp/mister-magik/launcher-return-state.json"
STALE_RETURN_STATE="/tmp/mister-magik/stale-launcher-return-state.json"
STATUS="/tmp/mister-magik/status.json"
MAIN_STATUS="/tmp/mister-magik/main-status.json"
REMOTE_LOG="/tmp/mister-magik-slint.log"
REMOTE_EVENTS="/tmp/mister-magik/events.jsonl"
SELECTED="${1:-17}"
GAME_SETTLE_SECS="${GAME_SETTLE_SECS:-8}"

if [[ ! "$SELECTED" =~ ^[0-9]+$ ]]; then
  echo "usage: scripts/device-launch-return-smoke.sh [selected-index]" >&2
  echo "selected-index must be a non-negative integer" >&2
  exit 2
fi

remote() {
  "$MISTER" run "$1"
}

send_main_command() {
  local command="$1"
  local timeout="${2:-5}"
  remote "$(mister_fifo_remote_command "$command" "$timeout")"
}

fifo_has_reader() {
  remote "for p in /proc/[0-9]*; do ls -l \"\$p/fd\" 2>/dev/null | grep -q /dev/MiSTer_cmd && exit 0; done; exit 1" >/dev/null 2>&1
}

launcher_is_active() {
  remote "grep -q '\"launcher_state\":\"LauncherActive\"' '$MAIN_STATUS' 2>/dev/null && grep -q '\"scene\":\"launcher\"' '$STATUS' 2>/dev/null" >/dev/null 2>&1
}

dump_failure_artifacts() {
  remote "echo MAIN; cat '$MAIN_STATUS' 2>/dev/null || true; echo STATUS; cat '$STATUS' 2>/dev/null || true; echo STATE; cat '$RETURN_STATE' 2>/dev/null || true; echo FBMODE; cat /sys/module/MiSTer_fb/parameters/mode 2>/dev/null || true; echo PROCS; ps w | grep -E 'MiSTer|MiSTer_MagiK|mister-magik-fb' | grep -v grep || true; echo LOG; tail -120 '$REMOTE_LOG' 2>/dev/null || true; echo EVENTS; tail -80 '$REMOTE_EVENTS' 2>/dev/null || true" >&2 || true
}

wait_remote() {
  local label="$1" timeout="$2" expr="$3"
  local elapsed=0
  while [ "$elapsed" -lt "$timeout" ]; do
    if remote "$expr" >/dev/null 2>&1; then
      echo "ok: $label"
      return 0
    fi
    sleep 1
    elapsed=$((elapsed + 1))
  done
  echo "FAIL: $label" >&2
  dump_failure_artifacts
  return 1
}

tmp_env="$(mktemp)"
cleanup() {
  rm -f "$tmp_env"
  remote "rm -f '$REMOTE_ENV'" >/dev/null 2>&1 || true
}
trap cleanup EXIT
cat >"$tmp_env" <<EOF
export MISTER_CATALOG_REFRESH=off
export MISTER_LAUNCHER_START_SCREEN=arcade
export MISTER_ARCADE_SELECTED_INDEX=$SELECTED
export MISTER_LAUNCHER_AUTO_LAUNCH_SELECTED=1
EOF

echo "==> Resetting launcher state"
remote "rm -f '$REMOTE_ENV' '$RETURN_STATE' '$STALE_RETURN_STATE' '$REMOTE_LOG' '$REMOTE_EVENTS'"
if ! launcher_is_active && ! fifo_has_reader; then
  echo "WARN: no active launcher or /dev/MiSTer_cmd reader; recovering with raw reboot"
  "$MISTER" reboot-wait --raw
elif ! send_main_command "mister_magik_restart_launcher"; then
  echo "WARN: launcher restart command could not write to /dev/MiSTer_cmd; recovering with raw reboot"
  "$MISTER" reboot-wait --raw
fi
wait_remote "launcher active before smoke" 60 "grep -q '\"launcher_state\":\"LauncherActive\"' '$MAIN_STATUS' 2>/dev/null && grep -q '\"scene\":\"launcher\"' '$STATUS' 2>/dev/null"

echo "==> Starting Arcade at selected row $SELECTED and auto-launching"
"$MISTER" put "$tmp_env" "$REMOTE_ENV" >/dev/null
send_main_command "mister_magik_restart_launcher"
wait_remote "launch return state written" 60 "test -s '$RETURN_STATE'"
remote "cat '$RETURN_STATE'"
wait_remote "return state recorded row $SELECTED" 5 "grep -q '\"game_index\": $SELECTED' '$RETURN_STATE' 2>/dev/null || grep -q '\"game_index\":$SELECTED' '$RETURN_STATE' 2>/dev/null"
remote "cp '$RETURN_STATE' '$STALE_RETURN_STATE'"
echo "==> Waiting ${GAME_SETTLE_SECS}s for active core handoff to settle"
sleep "$GAME_SETTLE_SECS"

echo "==> Returning from active core via load_core menu.rbf"
remote "rm -f '$REMOTE_ENV' '$REMOTE_LOG' '$REMOTE_EVENTS'"
if ! fifo_has_reader; then
  echo "WARN: active core has no /dev/MiSTer_cmd reader; using raw reboot return recovery"
  cat >"$tmp_env" <<EOF
export MISTER_MAGIK_RETURN_TO_LAUNCHER=1
EOF
  "$MISTER" put "$tmp_env" "$REMOTE_ENV" >/dev/null
  "$MISTER" reboot-wait --raw
elif ! send_main_command "load_core menu.rbf"; then
  echo "WARN: active core has no /dev/MiSTer_cmd reader; using raw reboot return recovery"
  cat >"$tmp_env" <<EOF
export MISTER_MAGIK_RETURN_TO_LAUNCHER=1
EOF
  "$MISTER" put "$tmp_env" "$REMOTE_ENV" >/dev/null
  "$MISTER" reboot-wait --raw
fi
wait_remote "launcher restored Arcade row $SELECTED" 90 "grep -q '\"screen\":\"arcade\"' '$STATUS' 2>/dev/null && grep -q '\"arcade_selected\":$SELECTED' '$STATUS' 2>/dev/null"
wait_remote "return state consumed" 10 "test ! -e '$RETURN_STATE'"

echo "==> Verifying stale return state is ignored without volatile return flag"
cat >"$tmp_env" <<EOF
export MISTER_MAGIK_RETURN_TO_LAUNCHER=0
EOF
"$MISTER" put "$tmp_env" "$REMOTE_ENV" >/dev/null
remote "cp '$STALE_RETURN_STATE' '$RETURN_STATE'"
send_main_command "mister_magik_restart_launcher"
wait_remote "stale return state ignored on normal launcher start" 60 "grep -q '\"screen\":\"home\"' '$STATUS' 2>/dev/null && grep -q '\"start_screen\":\"home\"' '$STATUS' 2>/dev/null"
wait_remote "stale return state consumed" 10 "test ! -e '$RETURN_STATE'"
remote "rm -f '$REMOTE_ENV' '$STALE_RETURN_STATE'"

echo "==> Device launch return smoke passed"
