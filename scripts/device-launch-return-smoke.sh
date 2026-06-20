#!/usr/bin/env bash
# Device smoke for launcher return state.
#
# Drives a real launcher-selected game handoff without controller injection:
#   1. start Arcade at a forced selected row
#   2. use a gated launcher env hook to launch that selected game
#   3. remove launcher.env
#   4. return from the running core with Main's active-core command:
#        load_core menu.rbf
#   5. assert MagiK consumes /tmp return state and restores Arcade at that row
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
MISTER="$HERE/scripts/mister"
REMOTE_ENV="/media/fat/mister-magik/launcher.env"
RETURN_STATE="/tmp/mister-magik/launcher-return-state.json"
STATUS="/tmp/mister-magik/status.json"
MAIN_STATUS="/tmp/mister-magik/main-status.json"
SELECTED="${1:-17}"

if [[ ! "$SELECTED" =~ ^[0-9]+$ ]]; then
  echo "usage: scripts/device-launch-return-smoke.sh [selected-index]" >&2
  echo "selected-index must be a non-negative integer" >&2
  exit 2
fi

remote() {
  "$MISTER" run "$1"
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
  remote "echo MAIN; cat '$MAIN_STATUS' 2>/dev/null || true; echo STATUS; cat '$STATUS' 2>/dev/null || true; echo STATE; cat '$RETURN_STATE' 2>/dev/null || true; echo PROCS; ps w | grep -E 'MiSTer_MagiK|mister-magik-fb' | grep -v grep || true" >&2 || true
  return 1
}

tmp_env="$(mktemp)"
trap 'rm -f "$tmp_env"' EXIT
cat >"$tmp_env" <<EOF
export MISTER_CATALOG_REFRESH=off
export MISTER_MAGIK_LIBRARY_REFRESH_DELAY_SECS=9999
export MISTER_LAUNCHER_START_SCREEN=arcade
export MISTER_ARCADE_SELECTED_INDEX=$SELECTED
export MISTER_LAUNCHER_AUTO_LAUNCH_SELECTED=1
EOF

echo "==> Resetting launcher state"
remote "rm -f '$REMOTE_ENV' '$RETURN_STATE'; printf 'mister_magik_restart_launcher\n' > /dev/MiSTer_cmd"
wait_remote "launcher active before smoke" 60 "grep -q '\"launcher_state\":\"LauncherActive\"' '$MAIN_STATUS' 2>/dev/null && grep -q '\"scene\":\"launcher\"' '$STATUS' 2>/dev/null"

echo "==> Starting Arcade at selected row $SELECTED and auto-launching"
"$MISTER" put "$tmp_env" "$REMOTE_ENV" >/dev/null
remote "printf 'mister_magik_restart_launcher\n' > /dev/MiSTer_cmd"
wait_remote "launch return state written" 60 "test -s '$RETURN_STATE'"
remote "cat '$RETURN_STATE'"
wait_remote "return state recorded row $SELECTED" 5 "grep -q '\"game_index\": $SELECTED' '$RETURN_STATE' 2>/dev/null || grep -q '\"game_index\":$SELECTED' '$RETURN_STATE' 2>/dev/null"

echo "==> Returning from active core via load_core menu.rbf"
remote "rm -f '$REMOTE_ENV'; printf 'load_core menu.rbf\n' > /dev/MiSTer_cmd"
wait_remote "launcher restored Arcade row $SELECTED" 90 "grep -q '\"screen\":\"arcade\"' '$STATUS' 2>/dev/null && grep -q '\"arcade_selected\":$SELECTED' '$STATUS' 2>/dev/null"
wait_remote "return state consumed" 10 "test ! -e '$RETURN_STATE'"

echo "==> Device launch return smoke passed"
