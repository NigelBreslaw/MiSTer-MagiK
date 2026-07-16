#!/usr/bin/env bash
# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

# Launch installed prepared collections through the production MagiK list and
# Main handoff, then return to MagiK without rebooting.
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
MISTER="$HERE/scripts/mister"
source "$HERE/scripts/lib/mister-fifo-lib.sh"
source "$HERE/scripts/lib/library-sql-output-lib.sh"

REMOTE_ENV="/media/fat/mister-magik-dev/launcher.env"
REMOTE_TEST_ENV="/tmp/mister-magik/prepared-launcher.env"
RETURN_STATE="/tmp/mister-magik/launcher-return-state.json"
STATUS="/tmp/mister-magik/status.json"
MAIN_STATUS="/tmp/mister-magik/main-status.json"
REMOTE_LOG="/tmp/mister-magik-slint.log"
REMOTE_EVENTS="/tmp/mister-magik/events.jsonl"
REMOTE_GATE="/tmp/mister-magik/prepared-launch.gate"
SETTLE_SECS="${PREPARED_LAUNCH_SETTLE_SECS:-5}"
CAPTURE_SECS="${PREPARED_LAUNCH_CAPTURE_SECS:-22}"
CAPTURE_DIR="$HERE/build/prepared-collection-acceptance"
ONLY_COLLECTION="${1:-all}"
ACTIVE_CASE=""
TMP_ENV="$(mktemp)"
PASSED=0
SKIPPED=0
CAMERA_PID=""
CAMERA_VIDEO=""

case "$ONLY_COLLECTION" in
  all|amigavision|0mhz|neon68k|oneload64) ;;
  *)
    echo "usage: scripts/device-prepared-collections-acceptance.sh [all|amigavision|0mhz|neon68k|oneload64]" >&2
    exit 2
    ;;
esac

remote() {
  "$MISTER" run "$1"
}

db() {
  "$MISTER" db "$1"
}

sql_string() {
  printf "'%s'" "${1//\'/\'\'}"
}

send_main_command() {
  remote "$(mister_fifo_remote_command "$1" "${2:-5}")"
}

fifo_has_reader() {
  remote "for p in /proc/[0-9]*; do ls -l \"\$p/fd\" 2>/dev/null | grep -q /dev/MiSTer_cmd && exit 0; done; exit 1" >/dev/null 2>&1
}

dump_failure_artifacts() {
  local capture_name="${ACTIVE_CASE:-prepared-failure}"
  mkdir -p "$CAPTURE_DIR"
  "$MISTER" agent framebuffer-capture "$CAPTURE_DIR/${capture_name}-failure-fb.png" \
    --json "$CAPTURE_DIR/${capture_name}-failure-fb.json" >&2 || true
  remote "echo MAIN; cat '$MAIN_STATUS' 2>/dev/null || true; echo STATUS; cat '$STATUS' 2>/dev/null || true; echo RETURN; cat '$RETURN_STATE' 2>/dev/null || true; echo PROCESSES; ps w | grep -E 'MiSTer|mister-magik' | grep -v grep || true; echo LOG; tail -160 '$REMOTE_LOG' 2>/dev/null || true; echo EVENTS; tail -160 '$REMOTE_EVENTS' 2>/dev/null || true" >&2 || true
}

wait_remote() {
  local label="$1" timeout="$2" expression="$3"
  local elapsed=0
  while [ "$elapsed" -lt "$timeout" ]; do
    if remote "$expression" >/dev/null 2>&1; then
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

launcher_active() {
  remote "grep -q '\"launcher_state\":\"LauncherActive\"' '$MAIN_STATUS' 2>/dev/null && grep -q '\"scene\":\"launcher\"' '$STATUS' 2>/dev/null" >/dev/null 2>&1
}

restore_launcher() {
  remote "rm -f '$REMOTE_ENV' '$REMOTE_TEST_ENV' '$REMOTE_GATE'" >/dev/null 2>&1 || true
  if launcher_active; then
    ACTIVE_CASE=""
    return 0
  fi
  if ! fifo_has_reader; then
    echo "FAIL: cannot return from ${ACTIVE_CASE:-prepared launch}; /dev/MiSTer_cmd has no reader (no reboot attempted)" >&2
    dump_failure_artifacts
    return 1
  fi
  send_main_command "load_core menu.rbf" 5 >/dev/null
  wait_remote "launcher restored after ${ACTIVE_CASE:-prepared launch}" 90 \
    "grep -q '\"launcher_state\":\"LauncherActive\"' '$MAIN_STATUS' 2>/dev/null && grep -q '\"scene\":\"launcher\"' '$STATUS' 2>/dev/null"
  ACTIVE_CASE=""
}

cleanup() {
  local status=$?
  rm -f "$TMP_ENV"
  remote "rm -f '$REMOTE_ENV' '$REMOTE_TEST_ENV' '$REMOTE_GATE'" >/dev/null 2>&1 || true
  if [ -n "$CAMERA_PID" ] && kill -0 "$CAMERA_PID" >/dev/null 2>&1; then
    kill "$CAMERA_PID" >/dev/null 2>&1 || true
    wait "$CAMERA_PID" >/dev/null 2>&1 || true
  fi
  if [ -n "$ACTIVE_CASE" ]; then
    restore_launcher || true
  fi
  return "$status"
}
trap cleanup EXIT INT TERM

collection_count() {
  db "SELECT count(*) FROM prepared_launch_rows WHERE collection_id=$(sql_string "$1");" |
    library_sql_first_result_number
}

candidate_line() {
  local collection="$1" where_sql="$2"
  candidate_line_at_offset "$collection" "$where_sql" 0
}

candidate_line_at_offset() {
  local collection="$1" where_sql="$2" offset="$3"
  db "SELECT p.launch_id,g.title,s.value,COALESCE(genre.value,''),l.ordinal
      FROM prepared_launch_rows p
      JOIN game_rows g ON g.game_key_id=p.launch_id
      JOIN string_values s ON s.string_id=g.system_string_id
      LEFT JOIN string_values genre ON genre.string_id=g.genre_string_id
      JOIN launcher_catalog_rows l ON l.launch_id=p.launch_id
      WHERE p.collection_id=$(sql_string "$collection") $where_sql
      ORDER BY l.ordinal LIMIT 1 OFFSET $offset;" | library_sql_first_result_line
}

candidate_for_title() {
  local collection="$1" title="$2"
  candidate_line "$collection" "AND g.title=$(sql_string "$title")"
}

system_index_before() {
  local system_id="$1" ordinal="$2"
  db "SELECT count(*)
      FROM launcher_catalog_rows l
      JOIN game_rows g ON g.game_key_id=l.launch_id
      JOIN string_values s ON s.string_id=g.system_string_id
      WHERE s.value=$(sql_string "$system_id") AND l.ordinal<$ordinal;" |
    library_sql_first_result_number
}

mgl_with_file_count() {
  local comparison="$1"
  remote "for f in '/media/fat/_DOS Games/'*.mgl; do n=\$(grep -c '<file' \"\$f\" 2>/dev/null || true); if [ \"\$n\" $comparison ]; then basename \"\$f\" .mgl; exit 0; fi; done; exit 1" |
    awk 'NF { print; exit }'
}

write_launch_env() {
  local system_id="$1" selected="$2"
  {
    printf 'export MISTER_LAUNCHER_START_SCREEN=arcade\n'
    printf 'export MISTER_LAUNCHER_START_SYSTEM=%q\n' "$system_id"
    printf 'export MISTER_ARCADE_SELECTED_INDEX=%q\n' "$selected"
    printf 'export MISTER_LAUNCHER_AUTO_LAUNCH_SELECTED=1\n'
    printf 'export MISTER_MAGIK_TEST_AUTO_LAUNCH_GATE=%q\n' "$REMOTE_GATE"
  } >"$TMP_ENV"
  "$MISTER" put "$TMP_ENV" "$REMOTE_TEST_ENV" >/dev/null
}

restart_launcher_with_test_env() {
  remote "set -e; trap 'rm -f '$REMOTE_ENV' '$REMOTE_TEST_ENV'' EXIT INT TERM; cp '$REMOTE_TEST_ENV' '$REMOTE_ENV'; $(mister_fifo_remote_command "mister_magik_restart_launcher" 5); waited=0; while [ \"\$waited\" -lt 30 ]; do pidof mister-magik-fb >/dev/null 2>&1 && exit 0; sleep 1; waited=\$((waited + 1)); done; exit 124"
}

capture_launcher_framebuffer() {
  local case_id="$1"
  mkdir -p "$CAPTURE_DIR"
  "$MISTER" agent framebuffer-capture "$CAPTURE_DIR/${case_id}-launcher.png" \
    --json "$CAPTURE_DIR/${case_id}-launcher.json" >/dev/null
  echo "ok: $case_id launcher framebuffer captured"
}

start_hdmi_capture() {
  local case_id="$1"
  mkdir -p "$CAPTURE_DIR"
  CAMERA_VIDEO="$CAPTURE_DIR/${case_id}-hdmi.mov"
  scripts/host-camera-native video --device-name "USB Video" --size 1920x1080 --fps 30 \
    --duration "$CAPTURE_SECS" --output "$CAMERA_VIDEO" \
    >"$CAPTURE_DIR/${case_id}-hdmi.log" 2>&1 &
  CAMERA_PID=$!
}

finish_hdmi_capture() {
  local case_id="$1"
  if [ -n "$CAMERA_PID" ]; then
    wait "$CAMERA_PID"
    CAMERA_PID=""
  fi
  test -s "$CAMERA_VIDEO"
  ffmpeg -hide_banner -loglevel error -y -sseof -1 -i "$CAMERA_VIDEO" \
    -frames:v 1 "$CAPTURE_DIR/${case_id}-hdmi.png"
  test -s "$CAPTURE_DIR/${case_id}-hdmi.png"
  echo "ok: $case_id HDMI capture recorded"
}

assert_amigavision_selector() {
  local title="$1"
  remote "found=0; for f in /media/fat/games/Amiga/shared/ags_boot /media/fat/games/Amiga/*/shared/ags_boot /media/fat/_Computer/*/shared/ags_boot; do if [ -f \"\$f\" ]; then found=1; value=\$(sed -n '1p' \"\$f\"); [ \"\$value\" = $(sql_string "$title") ] && exit 0; fi; done; [ \"\$found\" = 1 ] && exit 1; exit 2"
}

run_case() {
  local case_id="$1" collection="$2" line="$3" expected_handoff="$4"
  local launch_id title system_id genre ordinal selected
  IFS=$'\t' read -r launch_id title system_id genre ordinal <<<"$line"
  if [ -z "${launch_id:-}" ] || [ -z "${ordinal:-}" ]; then
    echo "FAIL: $case_id has no visible prepared candidate" >&2
    return 1
  fi
  selected="$(system_index_before "$system_id" "$ordinal")"
  echo "==> $case_id: title=$title system=$system_id index=$selected launch_id=$launch_id"

  restore_launcher
  remote "rm -f '$RETURN_STATE' '$REMOTE_LOG' '$REMOTE_EVENTS' '$REMOTE_GATE'" >/dev/null
  write_launch_env "$system_id" "$selected"
  ACTIVE_CASE="$case_id"
  restart_launcher_with_test_env >/dev/null

  wait_remote "$case_id visible system/index selected" 90 \
    "grep -q '\"screen\":\"arcade\"' '$STATUS' && grep -q '\"arcade_selected\":$selected' '$STATUS'"
  capture_launcher_framebuffer "$case_id"
  start_hdmi_capture "$case_id"
  remote "mkdir -p /tmp/mister-magik; printf 'ready\n' > '$REMOTE_GATE'"
  wait_remote "$case_id return context saved" 90 "test -s '$RETURN_STATE'"
  wait_remote "$case_id selected expected system/index" 10 \
    "grep -Eq '\"system_id\"[[:space:]]*:[[:space:]]*\"$system_id\"' '$RETURN_STATE' && grep -Eq '\"game_index\"[[:space:]]*:[[:space:]]*$selected([,}]|$)' '$RETURN_STATE'"
  wait_remote "$case_id Main handoff acknowledged" 45 \
    "grep -Fq $(sql_string "$expected_handoff") '$REMOTE_EVENTS' 2>/dev/null"
  wait_remote "$case_id launcher exited for active core" 20 \
    "! ps w | grep '[m]ister-magik-fb ui launcher' >/dev/null 2>&1"

  if [ "$collection" = "amigavision" ]; then
    if ! assert_amigavision_selector "$title"; then
      echo "FAIL: $case_id did not atomically select '$title'" >&2
      dump_failure_artifacts
      return 1
    fi
    echo "ok: $case_id selector contains exact title"
  fi

  sleep "$SETTLE_SECS"
  finish_hdmi_capture "$case_id"
  remote "rm -f '$REMOTE_ENV' '$REMOTE_TEST_ENV'" >/dev/null
  restore_launcher
  wait_remote "$case_id return state consumed" 15 "test ! -e '$RETURN_STATE'"
  PASSED=$((PASSED + 1))
  echo "PASS: $case_id"
}

run_amigavision() {
  local count game demo
  count="$(collection_count amigavision)"
  if [ "$count" -eq 0 ]; then
    echo "SKIP: amigavision is not installed"
    SKIPPED=$((SKIPPED + 2))
    return
  fi
  game="$(candidate_line amigavision "AND g.genre='AmigaVision'")"
  demo="$(candidate_line amigavision "AND g.genre='AmigaVision demos'")"
  run_case "amigavision-game" amigavision "$game" '"event":"handoff_launch"'
  run_case "amigavision-demo" amigavision "$demo" '"event":"handoff_launch"'
}

run_0mhz() {
  local count simple_title multi_title simple multi simple_path multi_path simple_before multi_before
  count="$(collection_count 0mhz)"
  if [ "$count" -eq 0 ]; then
    echo "SKIP: 0mhz is not installed"
    SKIPPED=$((SKIPPED + 2))
    return
  fi
  simple_title="$(mgl_with_file_count '-eq 1')"
  multi_title="$(mgl_with_file_count '-gt 1')"
  simple="$(candidate_for_title 0mhz "$simple_title")"
  multi="$(candidate_for_title 0mhz "$multi_title")"
  simple_path="/media/fat/_DOS Games/$simple_title.mgl"
  multi_path="/media/fat/_DOS Games/$multi_title.mgl"
  simple_before="$(remote "sha256sum $(sql_string "$simple_path") | awk '{print \$1}'")"
  multi_before="$(remote "sha256sum $(sql_string "$multi_path") | awk '{print \$1}'")"
  run_case "0mhz-simple" 0mhz "$simple" "\"event\":\"handoff_launch\",\"detail\":\"path=$simple_path\""
  run_case "0mhz-multi-image" 0mhz "$multi" "\"event\":\"handoff_launch\",\"detail\":\"path=$multi_path\""
  remote "[ \"\$(sha256sum $(sql_string "$simple_path") | awk '{print \$1}')\" = $(sql_string "$simple_before") ] && [ \"\$(sha256sum $(sql_string "$multi_path") | awk '{print \$1}')\" = $(sql_string "$multi_before") ]"
  echo "ok: 0MHz MGL files were handed to Main unchanged"
}

run_neon68k() {
  local count candidate path index
  count="$(collection_count neon68k)"
  if [ "$count" -eq 0 ]; then
    echo "SKIP: neon68k is not installed"
    SKIPPED=$((SKIPPED + 2))
    return
  fi
  if [ "$count" -lt 2 ]; then
    echo "FAIL: neon68k needs at least two prepared games, found $count" >&2
    return 1
  fi
  for index in 0 1; do
    candidate="$(candidate_line_at_offset neon68k '' "$index")"
    path="$(IFS=$'\t'; read -r launch_id title system_id genre ordinal <<<"$candidate"; "$MISTER" catalog launch-plan "$launch_id" | awk -F '\t' '$1 ~ /^[0-9]+$/ { print $10; exit }')"
    run_case "neon68k-game-$((index + 1))" neon68k "$candidate" "\"event\":\"handoff_launch\",\"detail\":\"path=$path\""
  done
}

run_oneload64_case() {
  local case_id="$1" where_sql="$2" candidate launch_id payload
  candidate="$(candidate_line oneload64 "$where_sql")"
  IFS=$'\t' read -r launch_id _ <<<"$candidate"
  payload="$("$MISTER" catalog launch-plan "$launch_id" | awk -F '\t' '$1 ~ /^[0-9]+$/ { print $9; exit }')"
  run_case "$case_id" oneload64 "$candidate" "\"event\":\"handoff_launch_plan\",\"detail\":\"core="
  remote "grep -Fq $(sql_string "payload=$payload") '$REMOTE_EVENTS'"
}

run_oneload64() {
  local count
  count="$(collection_count oneload64)"
  if [ "$count" -eq 0 ]; then
    echo "SKIP: oneload64 is not installed"
    SKIPPED=$((SKIPPED + 2))
    return
  fi
  # Keep two distinct future launch cases: one primary CRT and one MultiLoad64 CRT.
  run_oneload64_case "oneload64-primary" "AND g.game_id NOT LIKE '%/MultiLoad64/%'"
  run_oneload64_case "oneload64-multiload" "AND g.game_id LIKE '%/MultiLoad64/%'"
}

echo "==> Verifying device safety preconditions"
command -v ffmpeg >/dev/null
test -x scripts/host-camera-native
remote "test -p /dev/MiSTer_cmd; test -x /media/fat/mister-magik-dev/mister-magik-fb; rm -f '$REMOTE_ENV' '$REMOTE_GATE'"
wait_remote "MagiK launcher active before acceptance" 60 \
  "grep -q '\"launcher_state\":\"LauncherActive\"' '$MAIN_STATUS' 2>/dev/null && grep -q '\"scene\":\"launcher\"' '$STATUS' 2>/dev/null"

[ "$ONLY_COLLECTION" = all ] || echo "==> Restricted to $ONLY_COLLECTION"
if [ "$ONLY_COLLECTION" = all ] || [ "$ONLY_COLLECTION" = amigavision ]; then run_amigavision; fi
if [ "$ONLY_COLLECTION" = all ] || [ "$ONLY_COLLECTION" = 0mhz ]; then run_0mhz; fi
if [ "$ONLY_COLLECTION" = all ] || [ "$ONLY_COLLECTION" = neon68k ]; then run_neon68k; fi
if [ "$ONLY_COLLECTION" = all ] || [ "$ONLY_COLLECTION" = oneload64 ]; then run_oneload64; fi

restore_launcher
printf 'prepared_collection_acceptance_tsv\tpassed=%s\tskipped=%s\tresult=pass\n' \
  "$PASSED" "$SKIPPED"
