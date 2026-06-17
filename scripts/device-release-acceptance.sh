#!/usr/bin/env bash
# Canonical MiSTer hardware acceptance gate for public-beta releases.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
MISTER="$ROOT/scripts/mister"
STAMP="$(date -u +%Y%m%dT%H%M%SZ)"
OUT="$ROOT/build/device-release/$STAMP"
REMOTE_BIN="/media/fat/mister-magik/mister-magik-fb"
REMOTE_DB="/media/fat/mister-magik/library.sqlite3"
REMOTE_ENV="/media/fat/mister-magik/launcher.env"
REMOTE_ASSETS="/media/fat/mister-magik/assets"
LAUNCH_REF="${MISTER_ACCEPTANCE_LAUNCH_REF:-/media/fat/_Arcade/Missile Command (rev 3).mra}"
DEPLOY=0
ALLOW_RESET_CATALOG=0

usage() {
  cat <<'EOF'
usage: scripts/device-release-acceptance.sh [--skip-deploy|--deploy] [--allow-reset-catalog]

Runs the MiSTer hardware acceptance gate through scripts/mister only.

Options:
  --skip-deploy          Test the currently deployed device build. This is the default.
  --deploy               Build and deploy app + Main_MiSTer fork before testing.
  --allow-reset-catalog  Include destructive first-boot catalog recovery checks.
                         The existing library.sqlite3 is backed up first.

Environment:
  MISTER_ACCEPTANCE_LAUNCH_REF  Launch target for the game handoff smoke.
                                Default: /media/fat/_Arcade/Missile Command (rev 3).mra
EOF
}

for arg in "$@"; do
  case "$arg" in
    --skip-deploy) DEPLOY=0 ;;
    --deploy) DEPLOY=1 ;;
    --allow-reset-catalog) ALLOW_RESET_CATALOG=1 ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      echo "ERROR: unknown argument: $arg" >&2
      usage >&2
      exit 2
      ;;
  esac
done

mkdir -p "$OUT"
REPORT="$OUT/report.md"
FAILURES=0
FINISHED=0

append_report() {
  printf '%s\n' "$*" >> "$REPORT"
}

record_ok() {
  echo "ok: $*"
  append_report "- PASS: $*"
}

record_fail() {
  echo "FAIL: $*" >&2
  append_report "- FAIL: $*"
  FAILURES=$((FAILURES + 1))
}

run_capture() {
  local label="$1"
  shift
  echo "==> $label"
  append_report ""
  append_report "## $label"
  append_report ""
  if "$@" >"$OUT/$label.out" 2>"$OUT/$label.err"; then
    record_ok "$label"
  else
    record_fail "$label"
  fi
}

run_required_capture() {
  local label="$1"
  shift
  echo "==> $label"
  append_report ""
  append_report "## $label"
  append_report ""
  if "$@" >"$OUT/$label.out" 2>"$OUT/$label.err"; then
    record_ok "$label"
  else
    record_fail "$label"
    exit 1
  fi
}

wait_for_launcher_active() {
  local label="$1"
  local timeout="${2:-90}"
  local elapsed=0
  echo "==> $label"
  append_report ""
  append_report "## $label"
  append_report ""
  while [ "$elapsed" -le "$timeout" ]; do
    if remote "grep -q '\"launcher_state\":\"LauncherActive\"' /tmp/mister-magik/main-status.json 2>/dev/null && ps w | grep '[m]ister-magik-fb ui launcher' >/dev/null"; then
      record_ok "$label"
      return 0
    fi
    sleep 2
    elapsed=$((elapsed + 2))
  done
  record_fail "$label"
  return 1
}

remote() {
  "$MISTER" run "$1"
}

remote_get_optional() {
  local remote_path="$1"
  local local_name="$2"
  if "$MISTER" get "$remote_path" "$OUT/$local_name" >"$OUT/get-$local_name.out" 2>"$OUT/get-$local_name.err"; then
    record_ok "collected $remote_path"
  else
    printf '%s\n' "$remote_path" > "$OUT/$local_name.missing"
  fi
}

status_json() {
  "$MISTER" status --json > "$OUT/status-$1.json"
}

doctor_json() {
  "$MISTER" doctor --json > "$OUT/doctor-$1.json"
}

json_assert() {
  local file="$1"
  local label="$2"
  local expr="$3"
  python3 - "$file" "$label" "$expr" <<'PY'
import json
import sys

path, label, expr = sys.argv[1:4]
with open(path, "r", encoding="utf-8") as f:
    data = json.load(f)

try:
    builtins = {"all": all, "any": any, "int": int, "len": len, "str": str}
    ok = bool(eval(expr, {"__builtins__": builtins}, {"data": data}))
except Exception as exc:
    print(f"{label}: assertion error: {exc}", file=sys.stderr)
    sys.exit(2)

if not ok:
    print(f"{label}: assertion failed", file=sys.stderr)
    sys.exit(1)
PY
}

assert_status() {
  local file="$1"
  local label="$2"
  local expr="$3"
  if json_assert "$file" "$label" "$expr"; then
    record_ok "$label"
  else
    record_fail "$label"
  fi
}

last_number() {
  awk 'NF { value=$NF } END { gsub(/[^0-9]/, "", value); print value }'
}

assert_eq() {
  local label="$1"
  local expected="$2"
  local actual="$3"
  if [ "$actual" = "$expected" ]; then
    record_ok "$label = $actual"
  else
    record_fail "$label expected=$expected actual=${actual:-empty}"
  fi
}

assert_gt_zero() {
  local label="$1"
  local actual="$2"
  if [ -n "$actual" ] && [ "$actual" -gt 0 ]; then
    record_ok "$label = $actual"
  else
    record_fail "$label expected > 0 actual=${actual:-empty}"
  fi
}

restart_launcher() {
  remote "rm -f '$REMOTE_ENV'; if [ ! -p /dev/MiSTer_cmd ]; then echo 'missing /dev/MiSTer_cmd'; exit 12; fi; printf 'mister_magik_restart_launcher\n' > /dev/MiSTer_cmd"
  sleep 12
}

write_arcade_bench_env_and_restart() {
  remote "cat > '$REMOTE_ENV' <<'EOF'
export MISTER_LAUNCHER_START_SCREEN=arcade
export MISTER_LAUNCHER_LOCK_SCREEN=arcade
export MISTER_LAUNCHER_BENCH_SCENARIO=idle
export MISTER_PREVIEW_SCROLL_TRACE_SECS=5
EOF
if [ ! -p /dev/MiSTer_cmd ]; then echo 'missing /dev/MiSTer_cmd'; exit 12; fi
printf 'mister_magik_restart_launcher\n' > /dev/MiSTer_cmd"
  sleep 8
}

collect_artifacts() {
  status_json "final" || true
  doctor_json "final" || true
  "$MISTER" snapshot "$OUT/snapshot-final" >"$OUT/snapshot-final.out" 2>"$OUT/snapshot-final.err" || true
  remote_get_optional "/tmp/mister-magik/status.json" "slint-status.json"
  remote_get_optional "/tmp/mister-magik/main-status.json" "main-status.json"
  remote_get_optional "/tmp/mister-magik/events.jsonl" "events.jsonl"
  remote_get_optional "/tmp/mister-magik-slint.log" "slint.log"
  remote_get_optional "/tmp/mister-magik-main.log" "main.log"
  remote_get_optional "/tmp/mister-magik-launcher-frame-profile.tsv" "launcher-frame-profile.tsv"
  remote_get_optional "/tmp/mister-magik-visual-samples.tsv" "visual-samples.tsv"
}

finish() {
  local rc=$?
  if [ "$FINISHED" -eq 0 ]; then
    collect_artifacts || true
    append_report ""
    append_report "## Result"
    append_report ""
    append_report "FAIL (aborted with exit $rc before completion)"
    echo "device release acceptance: aborted (exit $rc)" >&2
    echo "artifacts: $OUT" >&2
  fi
  exit "$rc"
}

trap finish EXIT

cat > "$REPORT" <<EOF
# MiSTer MagiK Device Release Acceptance

- started: $STAMP
- launch_ref: $LAUNCH_REF
- deploy: $DEPLOY
- allow_reset_catalog: $ALLOW_RESET_CATALOG
- artifact_dir: $OUT
EOF

if [ "$DEPLOY" -eq 1 ]; then
  run_required_capture "deploy-main-mister-experiment" "$ROOT/scripts/deploy-main-mister-experiment.sh"
  run_required_capture "reboot-after-deploy" "$MISTER" reboot-wait
fi

run_required_capture "wait-device" "$MISTER" wait 120
wait_for_launcher_active "wait-launcher-initial" 90 || exit 1
run_capture "initial-status" "$MISTER" status
run_capture "initial-doctor" "$MISTER" doctor
status_json "initial" || {
  record_fail "initial status JSON"
  exit 1
}
doctor_json "initial" || {
  record_fail "initial doctor JSON"
  exit 1
}

assert_status "$OUT/doctor-initial.json" "doctor has no error findings" \
  "all(item[0] != 'error' for item in data.get('findings', []))"
assert_status "$OUT/status-initial.json" "boot main handoff is MiSTer_MagiK" \
  "data['boot']['ini_keys']['MiSTer']['main']['value'] == 'MiSTer_MagiK'"
assert_status "$OUT/status-initial.json" "active VT is tty2" \
  "data['display']['active_vt'] == 'tty2'"
assert_status "$OUT/status-initial.json" "framebuffer is RGB565 launcher mode" \
  "str(data['display']['fb_mode']).startswith('565 ')"
assert_status "$OUT/status-initial.json" "Main status is launcher active" \
  "data['runtime']['main_status'].get('launcher_state') == 'LauncherActive' and data['runtime']['main_status'].get('launcher_active') is True and int(data['runtime']['main_status'].get('launcher_pid', 0)) > 0"
assert_status "$OUT/status-initial.json" "Main invariant count is zero" \
  "int(data['runtime']['main_status'].get('invariant_count', 0)) == 0"
assert_status "$OUT/status-initial.json" "Slint launcher status is alive" \
  "data['runtime']['slint_status'].get('scene') == 'launcher' and int(data['runtime']['slint_status'].get('frames', 0)) > 0"
assert_status "$OUT/status-initial.json" "catalog is ready with games" \
  "data['runtime']['slint_status'].get('catalog_ready') is True and int(data['runtime']['slint_status'].get('catalog_games', 0)) > 0"

launcher_count="$(remote "ps w | grep '[m]ister-magik-fb ui launcher' | wc -l" | last_number || true)"
assert_eq "launcher process count" "1" "$launcher_count"

refresh_count="$(remote "ps w | grep '[m]ister-magik-fb library-refresh' | wc -l" | last_number || true)"
assert_eq "active library-refresh count" "0" "$refresh_count"

if remote "test -s '$REMOTE_DB'"; then
  record_ok "$REMOTE_DB is present and non-empty"
else
  record_fail "$REMOTE_DB is missing or empty"
fi

launcher_catalog_tables="$("$MISTER" db "SELECT count(*) FROM sqlite_master WHERE type='table' AND name='launcher_catalog';" | last_number || true)"
assert_eq "launcher_catalog table count" "1" "$launcher_catalog_tables"

console_pack_count="$(remote "ls '$REMOTE_ASSETS'/nes-screenshots.mmlz4b '$REMOTE_ASSETS'/snes-screenshots.mmlz4b '$REMOTE_ASSETS'/n64-screenshots.mmlz4b '$REMOTE_ASSETS'/sms-screenshots.mmlz4b '$REMOTE_ASSETS'/megadrive-screenshots.mmlz4b '$REMOTE_ASSETS'/saturn-screenshots.mmlz4b 2>/dev/null | wc -l" | last_number || true)"
if [ "${console_pack_count:-0}" -gt 0 ]; then
  canonical_assets="$("$MISTER" db "SELECT count(*) FROM asset_entries WHERE identity_namespace='mame-software';" | last_number || true)"
  assert_gt_zero "canonical mame-software asset entries" "$canonical_assets"
else
  record_ok "no console screenshot packs installed; canonical asset projection skipped"
fi

for platform in arcade neogeo saturn; do
  if remote "test -f '$REMOTE_ASSETS/${platform}-screenshots.mmlz4b'"; then
    count="$("$MISTER" db "SELECT COALESCE(SUM(has_image),0) FROM launcher_catalog WHERE platform_id='$platform';" | last_number || true)"
    assert_gt_zero "$platform has_image count" "$count"
  fi
done

if remote "test -f '$LAUNCH_REF'"; then
  record_ok "launch smoke target exists"
else
  record_fail "launch smoke target missing: $LAUNCH_REF"
fi

write_arcade_bench_env_and_restart
status_json "arcade-restart" || record_fail "arcade restart status JSON"
assert_status "$OUT/status-arcade-restart.json" "supervised restart reaches Arcade screen" \
  "data['runtime']['slint_status'].get('screen') == 'arcade'"
assert_status "$OUT/status-arcade-restart.json" "supervised restart keeps invariant count zero" \
  "int(data['runtime']['main_status'].get('invariant_count', 0)) == 0"

restart_launcher
status_json "normal-restart" || record_fail "normal restart status JSON"
assert_status "$OUT/status-normal-restart.json" "normal restart returns to launcher" \
  "data['runtime']['slint_status'].get('scene') == 'launcher'"

launcher_pid="$(python3 - "$OUT/status-normal-restart.json" <<'PY'
import json
import sys
with open(sys.argv[1], "r", encoding="utf-8") as f:
    status = json.load(f)
print(status["runtime"]["main_status"].get("launcher_pid", ""))
PY
)"
if [ -n "$launcher_pid" ] && [ "$launcher_pid" != "0" ]; then
  remote "kill -9 '$launcher_pid'"
  sleep 5
  status_json "crash-policy" || record_fail "crash policy status JSON"
  assert_status "$OUT/status-crash-policy.json" "killed Slint child is recorded as crash policy" \
    "data['runtime']['main_status'].get('launcher_state') == 'LauncherCrashed' and int(data['runtime']['main_status'].get('launcher_pid', 0)) == 0 and int(data['runtime']['main_status'].get('invariant_count', 0)) == 0"
else
  record_fail "could not determine launcher PID for crash-policy smoke"
fi

restart_launcher
status_json "post-crash-restart" || record_fail "post-crash restart status JSON"
assert_status "$OUT/status-post-crash-restart.json" "launcher restarts after crash-policy smoke" \
  "data['runtime']['main_status'].get('launcher_state') == 'LauncherActive' and int(data['runtime']['main_status'].get('launcher_pid', 0)) > 0 and data['runtime']['slint_status'].get('scene') == 'launcher'"
if ! json_assert "$OUT/status-post-crash-restart.json" "post-crash restart recovery needed" \
  "data['runtime']['main_status'].get('launcher_state') == 'LauncherActive' and int(data['runtime']['main_status'].get('launcher_pid', 0)) > 0" >/dev/null 2>&1; then
  run_capture "raw-reboot-after-crash-restart-failure" "$MISTER" reboot-wait --raw
  wait_for_launcher_active "wait-launcher-after-crash-raw-reboot" 90 || exit 1
  status_json "post-crash-raw-reboot" || record_fail "post crash raw reboot status JSON"
  assert_status "$OUT/status-post-crash-raw-reboot.json" "launcher recovers after crash-restart failure" \
    "data['runtime']['main_status'].get('launcher_state') == 'LauncherActive' and int(data['runtime']['main_status'].get('launcher_pid', 0)) > 0 and data['runtime']['slint_status'].get('scene') == 'launcher'"
fi

if [ "$ALLOW_RESET_CATALOG" -eq 1 ]; then
  BACKUP="/media/fat/mister-magik/library.sqlite3.acceptance-$STAMP.bak"
  remote "if [ -f '$REMOTE_DB' ]; then cp '$REMOTE_DB' '$BACKUP'; fi; rm -f '$REMOTE_DB'; sync"
  append_report ""
  append_report "catalog backup: $BACKUP"
  restart_launcher
  sleep 10
  status_json "catalog-reset" || record_fail "catalog reset status JSON"
  assert_status "$OUT/status-catalog-reset.json" "catalog reset shows launcher instead of black boot" \
    "data['runtime']['slint_status'].get('scene') == 'launcher'"
  remote "if [ -f '$BACKUP' ]; then mv '$BACKUP' '$REMOTE_DB'; fi; sync"
  restart_launcher
else
  record_ok "destructive catalog reset skipped"
fi

run_capture "supervised-reboot" "$MISTER" reboot-wait
wait_for_launcher_active "wait-launcher-after-supervised-reboot" 90 || exit 1
status_json "post-reboot" || record_fail "post-reboot status JSON"
assert_status "$OUT/status-post-reboot.json" "post-reboot launcher active" \
  "data['runtime']['main_status'].get('launcher_state') == 'LauncherActive' and int(data['runtime']['main_status'].get('launcher_pid', 0)) > 0 and data['runtime']['slint_status'].get('scene') == 'launcher'"
assert_status "$OUT/status-post-reboot.json" "post-reboot invariant count is zero" \
  "int(data['runtime']['main_status'].get('invariant_count', 0)) == 0"

remote "printf 'mister_magik_exit_to_menu\n' > /dev/MiSTer_cmd"
sleep 5
status_json "exit-menu" || record_fail "exit-to-menu status JSON"
assert_status "$OUT/status-exit-menu.json" "exit-to-menu reaches stock-menu handoff state" \
  "data['runtime']['main_status'].get('launcher_state') in ('HandoffToStockMenu', 'Unconfigured')"
run_capture "raw-reboot-after-exit-menu" "$MISTER" reboot-wait --raw
wait_for_launcher_active "wait-launcher-after-exit-menu-reboot" 90 || exit 1
status_json "post-exit-menu-reboot" || record_fail "post exit-menu reboot status JSON"
assert_status "$OUT/status-post-exit-menu-reboot.json" "launcher recovers after exit-to-menu smoke" \
  "data['runtime']['main_status'].get('launcher_state') == 'LauncherActive' and int(data['runtime']['main_status'].get('launcher_pid', 0)) > 0 and data['runtime']['slint_status'].get('scene') == 'launcher'"

remote "printf 'mister_magik_launch %s\n' '$LAUNCH_REF' > /dev/MiSTer_cmd"
sleep 10
status_json "handoff-game" || record_fail "handoff game status JSON"
assert_status "$OUT/status-handoff-game.json" "game handoff leaves active launcher state" \
  "data['runtime']['main_status'].get('launcher_state') in ('HandoffToGame', 'Unconfigured')"
run_capture "raw-reboot-after-game-handoff" "$MISTER" reboot-wait --raw
wait_for_launcher_active "wait-launcher-after-game-reboot" 90 || exit 1
status_json "post-game-reboot" || record_fail "post game reboot status JSON"
assert_status "$OUT/status-post-game-reboot.json" "launcher recovers after game handoff smoke" \
  "data['runtime']['main_status'].get('launcher_state') == 'LauncherActive' and int(data['runtime']['main_status'].get('launcher_pid', 0)) > 0 and data['runtime']['slint_status'].get('scene') == 'launcher'"

collect_artifacts
FINISHED=1
trap - EXIT

append_report ""
if [ "$FAILURES" -eq 0 ]; then
  append_report "## Result"
  append_report ""
  append_report "PASS"
  echo "device release acceptance: ok"
else
  append_report "## Result"
  append_report ""
  append_report "FAIL ($FAILURES failures)"
  echo "device release acceptance: FAIL ($FAILURES failures)" >&2
  echo "artifacts: $OUT" >&2
  exit 1
fi

echo "artifacts: $OUT"
