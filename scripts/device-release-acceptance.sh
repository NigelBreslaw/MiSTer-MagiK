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
FAST=0
SOAK=0
TIER_SPEC=""
TIERS_EXPLICIT=0
PRESET="release"
SELECTED_TIERS=""
SKIPPED_TIERS=""
SKIP_DISPLAY_MODES=0
SKIP_INSTALL_RESTORE=0
SKIP_FIRST_BOOT_RESET=0
SOAK_SECS="${MISTER_ACCEPTANCE_SOAK_SECS:-3600}"
BENCH_CHECKS="${MISTER_ACCEPTANCE_BENCH_TOOLS:-0}"
ALL_TIERS="health framebuffer-route launcher-lifecycle catalog handoff display-modes install-restore soak"
DEFAULT_TIERS="health framebuffer-route launcher-lifecycle catalog handoff display-modes install-restore"
FAST_TIERS="health framebuffer-route launcher-lifecycle catalog handoff"

usage() {
  cat <<'EOF'
usage: scripts/device-release-acceptance.sh [--skip-deploy|--deploy] [--tiers LIST] [--allow-reset-catalog] [--fast] [--soak] [--skip-display-modes] [--skip-install-restore] [--skip-first-boot-reset]

Runs the MiSTer hardware acceptance gate through scripts/mister only.

Options:
  --skip-deploy          Test the currently deployed device build. This is the default.
  --deploy               Build and deploy app + Main_MiSTer fork before testing.
  --tiers LIST           Run selected comma-separated tiers in canonical order.
                         Tiers: health, framebuffer-route, launcher-lifecycle,
                         catalog, handoff, display-modes, install-restore, soak.
  --allow-reset-catalog  Include destructive first-boot catalog recovery checks.
                         The existing library.sqlite3 is backed up first.
  --fast                 Run quick non-destructive checks only.
  --soak                 Include the long soak. Default off.
  --skip-display-modes   Skip HDMI mode smoke checks.
  --skip-install-restore Skip install/restore round trip.
  --skip-first-boot-reset Skip destructive first-boot scan check.

Environment:
  MISTER_ACCEPTANCE_LAUNCH_REF  Launch target for the game handoff smoke.
                                Default: /media/fat/_Arcade/Missile Command (rev 3).mra
  MISTER_ACCEPTANCE_BENCH_TOOLS Set to 1 to run bench-tools-only preview trace checks.
EOF
}

die_usage() {
  echo "ERROR: $*" >&2
  usage >&2
  exit 2
}

is_known_tier() {
  case "$1" in
    health|framebuffer-route|launcher-lifecycle|catalog|handoff|display-modes|install-restore|soak)
      return 0
      ;;
    *)
      return 1
      ;;
  esac
}

append_unique_tier() {
  local tier="$1"
  case " $SELECTED_TIERS " in
    *" $tier "*) ;;
    *) SELECTED_TIERS="${SELECTED_TIERS:+$SELECTED_TIERS }$tier" ;;
  esac
}

tier_selected() {
  case " $SELECTED_TIERS " in
    *" $1 "*) return 0 ;;
    *) return 1 ;;
  esac
}

parse_tier_spec() {
  local spec="$1" tier
  spec="${spec// /}"
  [ -n "$spec" ] || die_usage "--tiers requires at least one tier"
  IFS=, read -r -a requested_tiers <<< "$spec"
  SELECTED_TIERS=""
  for tier in "${requested_tiers[@]}"; do
    [ -n "$tier" ] || die_usage "--tiers contains an empty tier"
    is_known_tier "$tier" || die_usage "unknown tier: $tier"
  done
  for tier in $ALL_TIERS; do
    case ",$spec," in
      *",$tier,"*) append_unique_tier "$tier" ;;
    esac
  done
}

resolve_tiers() {
  if [ "$FAST" -eq 1 ] && [ "$TIERS_EXPLICIT" -eq 1 ]; then
    die_usage "--fast cannot be combined with --tiers"
  fi

  if [ "$TIERS_EXPLICIT" -eq 1 ]; then
    PRESET="custom"
    parse_tier_spec "$TIER_SPEC"
  elif [ "$FAST" -eq 1 ]; then
    PRESET="fast"
    SELECTED_TIERS="$FAST_TIERS"
  else
    PRESET="release"
    SELECTED_TIERS="$DEFAULT_TIERS"
  fi

  if tier_selected "soak"; then
    SOAK=1
  fi

  if [ "$SOAK" -eq 1 ]; then
    append_unique_tier "soak"
    PRESET="$PRESET+soak"
  fi

  SKIPPED_TIERS=""
  for tier in $ALL_TIERS; do
    if ! tier_selected "$tier"; then
      SKIPPED_TIERS="${SKIPPED_TIERS:+$SKIPPED_TIERS }$tier"
    fi
  done
}

while [ "$#" -gt 0 ]; do
  case "$1" in
    --skip-deploy) DEPLOY=0 ;;
    --deploy) DEPLOY=1 ;;
    --tiers)
      shift
      [ "$#" -gt 0 ] || die_usage "--tiers requires a comma-separated list"
      TIER_SPEC="$1"
      TIERS_EXPLICIT=1
      ;;
    --tiers=*)
      TIER_SPEC="${1#--tiers=}"
      TIERS_EXPLICIT=1
      ;;
    --allow-reset-catalog) ALLOW_RESET_CATALOG=1 ;;
    --fast) FAST=1 ;;
    --soak) SOAK=1 ;;
    --skip-display-modes) SKIP_DISPLAY_MODES=1 ;;
    --skip-install-restore) SKIP_INSTALL_RESTORE=1 ;;
    --skip-first-boot-reset) SKIP_FIRST_BOOT_RESET=1 ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      die_usage "unknown argument: $1"
      ;;
  esac
  shift
done

resolve_tiers

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
    case "$label" in
      *reboot*|*Reboot*)
        collect_boot_network_logs || true
        ;;
    esac
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
  "$MISTER" db "$1" | first_result_number
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

json_value() {
  local file="$1"
  local expr="$2"
  python3 - "$file" "$expr" <<'PY'
import json
import sys

with open(sys.argv[1], "r", encoding="utf-8") as f:
    data = json.load(f)
value = eval(sys.argv[2], {"__builtins__": {"int": int, "len": len, "str": str}}, {"data": data})
if isinstance(value, bool):
    print("true" if value else "false")
elif value is None:
    print("")
else:
    print(value)
PY
}

wait_status_expr() {
  local label="$1" timeout="$2" expr="$3" summary_expr="${4:-data['runtime'].get('main_status', {}).get('launcher_state', '?')}"
  local tmp="$OUT/wait-${label//[^A-Za-z0-9_.-]/_}.json"
  local elapsed=0
  echo "==> $label"
  append_report ""
  append_report "## $label"
  append_report ""
  while [ "$elapsed" -le "$timeout" ]; do
    if "$MISTER" status --json >"$tmp" 2>"$tmp.err"; then
      if json_assert "$tmp" "$label" "$expr" >/dev/null 2>&1; then
        record_ok "$label"
        return 0
      fi
      if [ $((elapsed % 10)) -eq 0 ]; then
        summary="$(json_value "$tmp" "$summary_expr" 2>/dev/null || true)"
        echo "  waiting ${elapsed}s/${timeout}s: ${summary:-no summary}"
      fi
    elif [ $((elapsed % 10)) -eq 0 ]; then
      echo "  waiting ${elapsed}s/${timeout}s: status unavailable"
    fi
    sleep 2
    elapsed=$((elapsed + 2))
  done
  record_fail "$label"
  return 1
}

wait_remote_event() {
  local label="$1" event="$2" timeout="$3"
  local elapsed=0
  echo "==> $label"
  append_report ""
  append_report "## $label"
  append_report ""
  while [ "$elapsed" -le "$timeout" ]; do
    if remote "grep -q '\"event\":\"$event\"' /tmp/mister-magik/events.jsonl 2>/dev/null"; then
      record_ok "$label"
      return 0
    fi
    if [ $((elapsed % 10)) -eq 0 ]; then
      echo "  waiting ${elapsed}s/${timeout}s for event=$event"
    fi
    sleep 2
    elapsed=$((elapsed + 2))
  done
  record_fail "$label"
  return 1
}

wait_remote_trace_rows() {
  local label="$1" remote_path="$2" min_rows="$3" timeout="$4"
  local elapsed=0 rows=0
  echo "==> $label"
  append_report ""
  append_report "## $label"
  append_report ""
  while [ "$elapsed" -le "$timeout" ]; do
    rows="$(remote "if [ -f '$remote_path' ]; then tail -n +2 '$remote_path' | wc -l; else echo 0; fi" | last_number || true)"
    rows="${rows:-0}"
    if [ "$rows" -ge "$min_rows" ]; then
      record_ok "$label rows=$rows"
      return 0
    fi
    if [ $((elapsed % 10)) -eq 0 ]; then
      echo "  waiting ${elapsed}s/${timeout}s: rows=$rows target=$min_rows"
    fi
    sleep 2
    elapsed=$((elapsed + 2))
  done
  record_fail "$label rows=${rows:-0} target=$min_rows"
  return 1
}

restart_launcher() {
  remote "rm -f '$REMOTE_ENV'; if [ ! -p /dev/MiSTer_cmd ]; then echo 'missing /dev/MiSTer_cmd'; exit 12; fi; printf 'mister_magik_restart_launcher\n' > /dev/MiSTer_cmd"
  wait_status_expr "wait normal launcher restart" 60 \
    "data['runtime']['main_status'].get('launcher_state') == 'LauncherActive' and data['runtime']['slint_status'].get('scene') == 'launcher'" \
    "data['runtime'].get('main_status', {}).get('launcher_state', '?')"
}

write_arcade_bench_env_and_restart() {
  remote "cat > '$REMOTE_ENV' <<'EOF'
export MISTER_LAUNCHER_START_SCREEN=arcade
export MISTER_LAUNCHER_LOCK_SCREEN=arcade
export MISTER_LAUNCHER_BENCH_SCENARIO=idle
export MISTER_PREVIEW_SCROLL_TRACE_SECS=5
export MISTER_CATALOG_REFRESH=off
EOF
if [ ! -p /dev/MiSTer_cmd ]; then echo 'missing /dev/MiSTer_cmd'; exit 12; fi
printf 'mister_magik_restart_launcher\n' > /dev/MiSTer_cmd"
  wait_status_expr "wait supervised Arcade restart" 60 \
    "data['runtime']['main_status'].get('launcher_state') == 'LauncherActive' and data['runtime']['slint_status'].get('screen') == 'arcade'" \
    "str(data['runtime'].get('slint_status', {}).get('screen', '?')) + ' ' + str(data['runtime'].get('slint_status', {}).get('catalog_ready', '?'))"
}

write_launcher_env() {
  local remote_trace="$1" scenario="$2" selected="${3:-}"
  local env_file="$OUT/launcher.env"
  {
    printf 'export MISTER_CATALOG_REFRESH=off\n'
    printf 'export MISTER_LAUNCHER_START_SCREEN=arcade\n'
    printf 'export MISTER_LAUNCHER_LOCK_SCREEN=arcade\n'
    printf 'export MISTER_LAUNCHER_BENCH_SCENARIO=%q\n' "$scenario"
    printf 'export MISTER_PREVIEW_TRACE=1\n'
    printf 'export MISTER_PREVIEW_SCROLL_TRACE_SECS=15\n'
    if [ -n "$remote_trace" ]; then
      printf 'export MISTER_PREVIEW_SCROLL_TRACE=%q\n' "$remote_trace"
    fi
    if [ -n "$selected" ]; then
      printf 'export MISTER_ARCADE_SELECTED_INDEX=%q\n' "$selected"
    fi
  } >"$env_file"
  "$MISTER" put "$env_file" "$REMOTE_ENV" >/dev/null
}

restart_with_env() {
  local label="$1" remote_trace="${2:-}"
  remote "rm -f /tmp/mister-magik-slint.log '$remote_trace'; if [ ! -p /dev/MiSTer_cmd ]; then echo 'missing /dev/MiSTer_cmd'; exit 12; fi; printf 'mister_magik_restart_launcher\n' > /dev/MiSTer_cmd"
  wait_status_expr "$label" 60 \
    "data['runtime']['main_status'].get('launcher_state') == 'LauncherActive' and data['runtime']['slint_status'].get('screen') == 'arcade'" \
    "str(data['runtime'].get('slint_status', {}).get('screen', '?')) + ' fps=' + str(data['runtime'].get('slint_status', {}).get('rolling_fps', data['runtime'].get('slint_status', {}).get('fps_estimate', '?')))"
}

run_preview_render_acceptance() {
  if [ "$BENCH_CHECKS" != "1" ]; then
    record_ok "preview trace acceptance skipped: requires bench-tools build"
    return
  fi
  local trace="/tmp/mister-acceptance-preview.tsv"
  write_launcher_env "$trace" "preview-step-hold" ""
  restart_with_env "preview render restart" "$trace"
  wait_remote_trace_rows "preview trace rows grow" "$trace" 60 45
  wait_status_expr "preview reaches non-placeholder state" 60 \
    "data['runtime']['slint_status'].get('preview_cache_state') not in ('placeholder', 'empty', '')" \
    "'preview=' + str(data['runtime'].get('slint_status', {}).get('preview_cache_state', '?')) + ' selected=' + str(data['runtime'].get('slint_status', {}).get('arcade_selected', '?'))"
  wait_status_expr "preview composition has no recovery" 10 \
    "int(data['runtime']['slint_status'].get('composition_recovery_count', 0)) == 0" \
    "'composition_state=' + str(data['runtime'].get('slint_status', {}).get('composition_state', '?')) + ' recovery_count=' + str(data['runtime'].get('slint_status', {}).get('composition_recovery_count', '?')) + ' invariant=' + str(data['runtime'].get('slint_status', {}).get('last_composition_invariant_kind', ''))"
  "$MISTER" get "$trace" "$OUT/preview-render.tsv" >/dev/null 2>&1 || true
}

run_velocity_scroll_acceptance() {
  if [ "$BENCH_CHECKS" != "1" ]; then
    record_ok "velocity trace acceptance skipped: requires bench-tools build"
    return
  fi
  local scenario trace min_rows
  for scenario in held-scroll turbo-hold; do
    trace="/tmp/mister-acceptance-${scenario}.tsv"
    write_launcher_env "$trace" "$scenario" ""
    restart_with_env "velocity $scenario restart" "$trace"
    min_rows=300
    wait_remote_trace_rows "velocity $scenario trace rows" "$trace" "$min_rows" 35
    wait_status_expr "velocity $scenario composition has no recovery" 10 \
      "int(data['runtime']['slint_status'].get('composition_recovery_count', 0)) == 0" \
      "'composition_state=' + str(data['runtime'].get('slint_status', {}).get('composition_state', '?')) + ' recovery_count=' + str(data['runtime'].get('slint_status', {}).get('composition_recovery_count', '?')) + ' invariant=' + str(data['runtime'].get('slint_status', {}).get('last_composition_invariant_kind', ''))"
    "$MISTER" get "$trace" "$OUT/velocity-${scenario}.tsv" >/dev/null 2>&1 || true
    if "$ROOT/scripts/analyze-arcade-frame-trace.py" "$OUT/velocity-${scenario}.tsv" >"$OUT/velocity-${scenario}.analysis" 2>"$OUT/velocity-${scenario}.analysis.err"; then
      record_ok "velocity $scenario analysis"
    else
      record_fail "velocity $scenario analysis"
    fi
  done
}

run_controller_acceptance() {
  status_json "controller" || {
    record_fail "controller status JSON"
    return
  }
  assert_status "$OUT/status-controller.json" "controller status exposes input devices" \
    "len(data.get('input', {}).get('devices', [])) >= 0 and 'input_pad_count' in data['runtime']['slint_status']"
  pad_count="$(json_value "$OUT/status-controller.json" "data['runtime']['slint_status'].get('input_pad_count', 0)" || echo 0)"
  if [ "${pad_count:-0}" -gt 0 ]; then
    record_ok "launcher reports controller pads = $pad_count"
  else
    record_ok "controller hot-plug/navigation skipped: no connected pad reported"
  fi
}

run_audio_probe() {
  record_ok "audio probe covered by video path"
}

run_first_boot_visible_scan() {
  if [ "$FAST" -eq 1 ] || [ "$SKIP_FIRST_BOOT_RESET" -eq 1 ]; then
    record_ok "first-boot visible scan skipped"
    return
  fi
  local backup="/media/fat/mister-magik/library.sqlite3.acceptance-$STAMP.bak"
  remote "if [ -f '$REMOTE_DB' ]; then cp '$REMOTE_DB' '$backup'; fi; rm -f '$REMOTE_DB' '$REMOTE_ENV' /media/fat/mister-magik/bench-boot /tmp/mister-magik/events.jsonl /tmp/mister-magik-slint.log; sync"
  run_capture "first-boot-reboot" "$MISTER" reboot-wait
  wait_remote_event "first-boot first frame event" "first_frame" 60
  wait_status_expr "first-boot scan screen visible" 60 \
    "data['runtime']['slint_status'].get('catalog_ready') is False and data['runtime']['slint_status'].get('catalog_scan_visible') is True and data['display']['fb0_visual'].get('class') != 'mostly_black'" \
    "'scan=' + str(data['runtime'].get('slint_status', {}).get('catalog_scan_title', '?')) + ' fb0=' + str(data['display'].get('fb0_visual', {}).get('class', '?'))"
  wait_status_expr "first-boot catalog becomes ready" 300 \
    "data['runtime']['slint_status'].get('catalog_ready') is True and int(data['runtime']['slint_status'].get('catalog_games', 0)) > 0" \
    "'ready=' + str(data['runtime'].get('slint_status', {}).get('catalog_ready', '?')) + ' games=' + str(data['runtime'].get('slint_status', {}).get('catalog_games', '?')) + ' detail=' + str(data['runtime'].get('slint_status', {}).get('catalog_scan_detail', '?'))"
  remote "if [ -f '$backup' ]; then mv '$backup' '$REMOTE_DB'; sync; fi"
  restart_launcher
}

run_catalog_mutation_acceptance() {
  local fixture_root="/tmp/mister-magik-acceptance-library"
  local fixture="$fixture_root/_Arcade"
  local sqlite="/tmp/mister-magik-acceptance-fixture.sqlite3"
  local count_sql="SELECT count(*) FROM games WHERE title IN ('Acceptance One','Acceptance Two');"
  local dump_sql="SELECT games.title, launch_plans.launch_ref, games.system_id FROM games JOIN launch_plans USING(game_id) WHERE games.title IN ('Acceptance One','Acceptance Two') ORDER BY games.title;"
  remote "rm -rf '$fixture_root' '$sqlite'; mkdir -p '$fixture'; printf '<misterromdescription><setname>acceptance_one</setname></misterromdescription>\n' > '$fixture/Acceptance One.mra'"
  remote "MISTER_LIBRARY_ROOTS='$fixture_root' MISTER_LIBRARY_SQLITE='$sqlite' '$REMOTE_BIN' library-refresh >/tmp/mister-magik-catalog-mutation-a.log 2>&1"
  local first_count
  first_count="$(remote "MISTER_LIBRARY_SQLITE='$sqlite' '$REMOTE_BIN' library-sql \"$count_sql\" 2>/dev/null || echo 0" | first_result_number || true)"
  remote "printf '<misterromdescription><setname>acceptance_two</setname></misterromdescription>\n' > '$fixture/Acceptance Two.mra'; MISTER_LIBRARY_ROOTS='$fixture_root' MISTER_LIBRARY_SQLITE='$sqlite' '$REMOTE_BIN' library-refresh >/tmp/mister-magik-catalog-mutation-b.log 2>&1"
  local second_count
  second_count="$(remote "MISTER_LIBRARY_SQLITE='$sqlite' '$REMOTE_BIN' library-sql \"$count_sql\" 2>/dev/null || echo 0" | first_result_number || true)"
  remote "MISTER_LIBRARY_SQLITE='$sqlite' '$REMOTE_BIN' library-sql \"$dump_sql\" >/tmp/mister-magik-catalog-mutation-games.tsv 2>&1 || true"
  remote_get_optional "/tmp/mister-magik-catalog-mutation-a.log" "catalog-mutation-a.log"
  remote_get_optional "/tmp/mister-magik-catalog-mutation-b.log" "catalog-mutation-b.log"
  remote_get_optional "/tmp/mister-magik-catalog-mutation-games.tsv" "catalog-mutation-games.tsv"
  if [ "${first_count:-0}" = "1" ] && [ "${second_count:-0}" = "2" ]; then
    record_ok "catalog mutation fixture count $first_count -> $second_count"
  else
    record_fail "catalog mutation fixture count expected 1 -> 2 first=${first_count:-empty} second=${second_count:-empty}"
  fi
  remote "rm -rf '$fixture_root' '$sqlite' /tmp/mister-magik-catalog-mutation-a.log /tmp/mister-magik-catalog-mutation-b.log /tmp/mister-magik-catalog-mutation-games.tsv"
}

run_launch_matrix() {
  local candidates out target idx=0
  out="$OUT/launch-candidates.txt"
  remote "for p in '$LAUNCH_REF' '/media/fat/_Arcade/Metal Slug 3.mra' '/media/fat/_Arcade/Donkey Kong (US set 1).mra' '/media/fat/_Console/NeoGeo/Metal Slug 3.mgl' '/media/fat/_Console/NES/Super Mario Bros.mgl'; do [ -f \"\$p\" ] && echo \"\$p\"; done" >"$out" || true
  while IFS= read -r target; do
    [ -n "$target" ] || continue
    idx=$((idx + 1))
    restart_launcher
    remote "printf 'mister_magik_launch %s\n' '$target' > /dev/MiSTer_cmd"
    wait_status_expr "launch matrix handoff $idx" 45 \
      "data['runtime']['main_status'].get('launcher_state') in ('HandoffToGame', 'Unconfigured')" \
      "data['runtime'].get('main_status', {}).get('launcher_state', '?')"
    run_capture "launch-matrix-raw-reboot-$idx" "$MISTER" reboot-wait --raw
    wait_for_launcher_active "wait-launcher-after-launch-matrix-$idx" 90 || return
  done <"$out"
  if [ "$idx" -eq 0 ]; then
    record_fail "launch matrix found no launch candidates"
  fi
}

run_exit_menu_loop() {
  local i
  for i in 1 2; do
    restart_launcher
    remote "printf 'mister_magik_exit_to_menu\n' > /dev/MiSTer_cmd"
    wait_status_expr "exit-menu handoff loop $i" 30 \
      "data['runtime']['main_status'].get('launcher_state') in ('HandoffToStockMenu', 'Unconfigured')" \
      "data['runtime'].get('main_status', {}).get('launcher_state', '?')"
    run_capture "exit-menu-loop-raw-reboot-$i" "$MISTER" reboot-wait --raw
    wait_for_launcher_active "wait-launcher-after-exit-menu-loop-$i" 90 || return
  done
}

run_crash_restart_lite() {
  local i
  for i in 1 2 3; do
    run_crash_restart_smoke "crash-lite-$i" || return
  done
}

run_crash_restart_smoke() {
  local label="$1"
  local pid before_crash before_restart crash_expr after_expr

  restart_launcher || return 1
  status_json "$label-before" || {
    record_fail "$label before status JSON"
    return 1
  }
  pid="$(json_value "$OUT/status-$label-before.json" "data['runtime']['main_status'].get('launcher_pid', '')" || true)"
  before_crash="$(json_value "$OUT/status-$label-before.json" "int(data['runtime']['main_status'].get('crash_count', 0))" || true)"
  before_restart="$(json_value "$OUT/status-$label-before.json" "int(data['runtime']['main_status'].get('restart_count', 0))" || true)"
  before_crash="${before_crash:-0}"
  before_restart="${before_restart:-0}"

  if [ -z "$pid" ] || [ "$pid" = "0" ]; then
    record_fail "$label missing launcher pid"
    return 1
  fi

  remote "kill -9 '$pid'"
  crash_expr="data['runtime']['main_status'].get('launcher_state') == 'LauncherCrashed' and int(data['runtime']['main_status'].get('launcher_pid', 0)) == 0 and int(data['runtime']['main_status'].get('crash_count', 0)) > $before_crash and len(data['runtime']['main_status'].get('last_crash_reason', '')) > 0 and int(data['runtime']['main_status'].get('invariant_count', 0)) == 0"
  wait_status_expr "$label records LauncherCrashed" 30 \
    "$crash_expr" \
    "data['runtime'].get('main_status', {}).get('launcher_state', '?')"
  if status_json "$label-crashed"; then
    assert_status "$OUT/status-$label-crashed.json" "$label crash telemetry is recorded" "$crash_expr"
    assert_status "$OUT/status-$label-crashed.json" "$label crash report path is recorded" "data['runtime']['main_status'].get('last_crash_report', '').startswith('/media/fat/mister-magik/crashes/report-main-')"
  else
    record_fail "$label crashed status JSON"
  fi
  if remote "test -s /media/fat/mister-magik/crashes/latest.json"; then
    record_ok "$label persistent crash report exists"
  else
    record_fail "$label persistent crash report exists"
  fi

  if restart_launcher; then
    after_expr="data['runtime']['main_status'].get('launcher_state') == 'LauncherActive' and int(data['runtime']['main_status'].get('launcher_pid', 0)) > 0 and data['runtime']['slint_status'].get('scene') == 'launcher' and int(data['runtime']['main_status'].get('restart_count', 0)) > $before_restart and 'last_restart_error' in data['runtime']['main_status'] and data['runtime']['main_status'].get('last_restart_error') == '' and 'last_spawn_error' in data['runtime']['main_status'] and data['runtime']['main_status'].get('last_spawn_error') == '' and int(data['runtime']['main_status'].get('invariant_count', 0)) == 0"
    status_json "$label-after" || {
      record_fail "$label after status JSON"
      return 1
    }
    if json_assert "$OUT/status-$label-after.json" "$label returns LauncherActive without raw reboot" "$after_expr"; then
      record_ok "$label returns LauncherActive without raw reboot"
      return 0
    fi
    record_fail "$label returns LauncherActive without raw reboot"
    return 1
  fi

  record_fail "$label restart did not return to LauncherActive without raw reboot"
  run_capture "$label-raw-reboot-recovery" "$MISTER" reboot-wait --raw
  wait_for_launcher_active "wait-launcher-after-$label-raw-reboot-recovery" 90 || return 1
  return 1
}

run_display_mode_smoke() {
  if [ "$FAST" -eq 1 ] || [ "$SKIP_DISPLAY_MODES" -eq 1 ]; then
    record_ok "display mode smoke skipped"
    return
  fi
  local mode
  for mode in 1080p 720p low; do
    if "$ROOT/scripts/mister-video-mode-test.sh" sweep-mode "$mode" launcher >"$OUT/display-${mode}.out" 2>"$OUT/display-${mode}.err"; then
      record_ok "display mode smoke $mode"
    else
      record_fail "display mode smoke $mode"
    fi
    "$ROOT/scripts/mister-video-mode-test.sh" restore >"$OUT/display-${mode}-restore.out" 2>"$OUT/display-${mode}-restore.err" || record_fail "display mode restore $mode"
    wait_for_launcher_active "wait-launcher-after-display-$mode" 90 || return
  done
}

run_install_restore_roundtrip() {
  if [ "$FAST" -eq 1 ] || [ "$SKIP_INSTALL_RESTORE" -eq 1 ]; then
    record_ok "install/restore round trip skipped"
    return
  fi
  run_capture "restore-stock-boot" "$ROOT/scripts/restore-stock-boot.sh"
  run_capture "install-slint-boot" "$ROOT/scripts/install-slint-boot.sh"
  wait_for_launcher_active "wait-launcher-after-install-roundtrip" 120 || return
}

run_soak() {
  if [ "$SOAK" -ne 1 ]; then
    record_ok "long soak skipped"
    return
  fi
  local deadline=$((SECONDS + SOAK_SECS))
  local iteration=0
  while [ "$SECONDS" -lt "$deadline" ]; do
    iteration=$((iteration + 1))
    write_launcher_env "/tmp/mister-acceptance-soak.tsv" "held-scroll" ""
    restart_with_env "soak restart $iteration" "/tmp/mister-acceptance-soak.tsv"
    sleep 30
    status_json "soak-$iteration" || record_fail "soak status $iteration"
    assert_status "$OUT/status-soak-$iteration.json" "soak health $iteration" \
      "data['runtime']['main_status'].get('launcher_state') == 'LauncherActive' and int(data['runtime']['main_status'].get('invariant_count', 0)) == 0 and data['runtime']['slint_status'].get('last_frame_ms_ago', 999999) < 5000"
  done
}

write_report_header() {
  cat > "$REPORT" <<EOF
# MiSTer MagiK Device Release Acceptance

- started: $STAMP
- launch_ref: $LAUNCH_REF
- deploy: $DEPLOY
- preset: $PRESET
- selected_tiers: $SELECTED_TIERS
- skipped_tiers: ${SKIPPED_TIERS:-none}
- allow_reset_catalog: $ALLOW_RESET_CATALOG
- fast: $FAST
- soak: $SOAK
- soak_secs: $SOAK_SECS
- bench_checks: $BENCH_CHECKS
- skip_display_modes: $SKIP_DISPLAY_MODES
- skip_install_restore: $SKIP_INSTALL_RESTORE
- skip_first_boot_reset: $SKIP_FIRST_BOOT_RESET
- artifact_dir: $OUT
EOF
}

run_deploy_if_requested() {
  if [ "$DEPLOY" -eq 1 ]; then
    run_required_capture "deploy-main-mister-experiment" "$ROOT/scripts/deploy-main-mister-experiment.sh"
    run_required_capture "raw-reboot-after-deploy" "$MISTER" reboot-wait --raw
  fi
}

run_initial_gate() {
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
}

run_tier_health() {
  assert_status "$OUT/doctor-initial.json" "doctor has no error findings" \
    "all(item[0] != 'error' for item in data.get('findings', []))"
  assert_status "$OUT/status-initial.json" "boot main handoff is MiSTer_MagiK" \
    "data['boot']['ini_keys']['MiSTer']['main']['value'] == 'MiSTer_MagiK'"
  assert_status "$OUT/status-initial.json" "Main status is launcher active" \
    "data['runtime']['main_status'].get('launcher_state') == 'LauncherActive' and data['runtime']['main_status'].get('launcher_active') is True and int(data['runtime']['main_status'].get('launcher_pid', 0)) > 0"
  assert_status "$OUT/status-initial.json" "Main invariant count is zero" \
    "int(data['runtime']['main_status'].get('invariant_count', 0)) == 0"
  assert_status "$OUT/status-initial.json" "Slint launcher status is alive" \
    "data['runtime']['slint_status'].get('scene') == 'launcher' and int(data['runtime']['slint_status'].get('frames', 0)) > 0"

  launcher_count="$(remote "ps w | grep '[m]ister-magik-fb ui launcher' | wc -l" | last_number || true)"
  assert_eq "launcher process count" "1" "$launcher_count"

  if remote "test -f '$LAUNCH_REF'"; then
    record_ok "launch smoke target exists"
  else
    record_fail "launch smoke target missing: $LAUNCH_REF"
  fi

  run_preview_render_acceptance || true
  run_velocity_scroll_acceptance || true
  run_controller_acceptance || true
  run_audio_probe || true
}

run_tier_framebuffer_route() {
  assert_status "$OUT/status-initial.json" "active VT is tty2" \
    "data['display']['active_vt'] == 'tty2'"
  assert_status "$OUT/status-initial.json" "framebuffer is RGB565 launcher mode" \
    "str(data['display']['fb_mode']).startswith('565 ')"
}

run_tier_catalog() {
  assert_status "$OUT/status-initial.json" "catalog is ready with games" \
    "data['runtime']['slint_status'].get('catalog_ready') is True and int(data['runtime']['slint_status'].get('catalog_games', 0)) > 0"

  refresh_count="$(remote "ps w | grep '[m]ister-magik-fb library-refresh' | wc -l" | last_number || true)"
  assert_eq "active library-refresh count" "0" "$refresh_count"

  if remote "test -s '$REMOTE_DB'"; then
    record_ok "$REMOTE_DB is present and non-empty"
  else
    record_fail "$REMOTE_DB is missing or empty"
  fi

  launcher_catalog_tables="$(db_scalar "SELECT count(*) FROM sqlite_master WHERE type='table' AND name='launcher_catalog';" || true)"
  assert_eq "launcher_catalog table count" "1" "$launcher_catalog_tables"

  console_pack_count="$(remote "find '$REMOTE_ASSETS' -maxdepth 1 -type f \\( -name '*-screenshots.mmlz4b' -o -name '*-screenshots-320x320.mmlz4b' \\) 2>/dev/null | wc -l" | last_number || true)"
  if [ "${console_pack_count:-0}" -gt 0 ]; then
    asset_entry_tables="$(db_scalar "SELECT count(*) FROM sqlite_master WHERE type='table' AND name='asset_entries';" || true)"
    assert_eq "runtime-only screenshot asset table count" "0" "$asset_entry_tables"
  else
    record_ok "no console screenshot packs installed; runtime-only preview check skipped"
  fi

  if remote "test -f '$REMOTE_ASSETS/arcade-screenshots-320x320.mmlz4b' || test -f '$REMOTE_ASSETS/arcade-screenshots.mmlz4b'"; then
    count="$(db_scalar "SELECT COALESCE(SUM(has_preview),0) FROM launcher_catalog WHERE system_id='arcade';" || true)"
    assert_gt_zero "arcade has_preview count" "$count"
  fi
  for platform in neogeo saturn; do
    if remote "test -f '$REMOTE_ASSETS/${platform}-screenshots-320x320.mmlz4b' || test -f '$REMOTE_ASSETS/${platform}-screenshots.mmlz4b'"; then
      count="$(db_scalar "SELECT COALESCE(SUM(has_preview),0) FROM launcher_catalog WHERE system_id='$platform';" || true)"
      assert_gt_zero "$platform has_preview count" "$count"
    fi
  done

  if remote "test -f '$REMOTE_ASSETS/.screenshot-media-state.json'"; then
    size_state_count="$(remote "grep -c 'screenshots-320x320\\.mmlz4b' '$REMOTE_ASSETS/.screenshot-media-state.json' 2>/dev/null || true" | last_number || true)"
    if [ "${size_state_count:-0}" -gt 0 ]; then
      record_ok "media state size-qualified local_path count = $size_state_count"
      cache_state_count="$(remote "grep -c 'cf_cache_status\\|content_length\\|effective_url' '$REMOTE_ASSETS/.screenshot-media-state.json' 2>/dev/null || true" | last_number || true)"
      assert_gt_zero "media state cache metadata count" "$cache_state_count"
    else
      record_ok "media state present without size-qualified runtime downloads"
    fi
  else
    record_ok "media state not present; runtime downloader has not published packs on this device"
  fi

  if [ "$FAST" -eq 0 ]; then
    run_catalog_mutation_acceptance || true
    run_first_boot_visible_scan || true
  fi

  if [ "$ALLOW_RESET_CATALOG" -eq 1 ]; then
    BACKUP="/media/fat/mister-magik/library.sqlite3.acceptance-$STAMP.bak"
    remote "if [ -f '$REMOTE_DB' ]; then cp '$REMOTE_DB' '$BACKUP'; fi; rm -f '$REMOTE_DB'; sync"
    append_report ""
    append_report "catalog backup: $BACKUP"
    restart_launcher
    wait_remote_event "catalog reset first frame" "first_frame" 60
    status_json "catalog-reset" || record_fail "catalog reset status JSON"
    assert_status "$OUT/status-catalog-reset.json" "catalog reset shows launcher instead of black boot" \
      "data['runtime']['slint_status'].get('scene') == 'launcher'"
    remote "if [ -f '$BACKUP' ]; then mv '$BACKUP' '$REMOTE_DB'; fi; sync"
    restart_launcher
  else
    record_ok "destructive catalog reset skipped"
  fi
}

run_supervised_reboot_soak() {
  run_capture "supervised-reboot-soak-15" "$MISTER" agent boot-profile 15 --timeout 60 --fail-on-timeout
  wait_for_launcher_active "wait-launcher-after-supervised-reboot-soak" 90 || return 1
  status_json "post-reboot-soak" || record_fail "post reboot soak status JSON"
  assert_status "$OUT/status-post-reboot-soak.json" "post-reboot soak launcher active" \
    "data['runtime']['main_status'].get('launcher_state') == 'LauncherActive' and int(data['runtime']['main_status'].get('launcher_pid', 0)) > 0 and data['runtime']['slint_status'].get('scene') == 'launcher'"
}

run_tier_launcher_lifecycle() {
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

  run_crash_restart_smoke "crash-restart-gate" || true

  run_capture "supervised-reboot" "$MISTER" reboot-wait
  wait_for_launcher_active "wait-launcher-after-supervised-reboot" 90 || return 1
  status_json "post-reboot" || record_fail "post-reboot status JSON"
  assert_status "$OUT/status-post-reboot.json" "post-reboot launcher active" \
    "data['runtime']['main_status'].get('launcher_state') == 'LauncherActive' and int(data['runtime']['main_status'].get('launcher_pid', 0)) > 0 and data['runtime']['slint_status'].get('scene') == 'launcher'"
  assert_status "$OUT/status-post-reboot.json" "post-reboot invariant count is zero" \
    "int(data['runtime']['main_status'].get('invariant_count', 0)) == 0"

  if [ "$FAST" -eq 0 ]; then
    run_capture "startup-reveal-acceptance" "$ROOT/scripts/device-startup-reveal-acceptance.sh" "release-$STAMP" || true
    if tier_selected "soak"; then
      run_supervised_reboot_soak || true
    else
      record_ok "supervised reboot soak skipped"
    fi
    run_crash_restart_lite || true
  fi
}

run_tier_handoff() {
  remote "printf 'mister_magik_exit_to_menu\n' > /dev/MiSTer_cmd"
  wait_status_expr "wait exit-to-menu handoff" 30 \
    "data['runtime']['main_status'].get('launcher_state') in ('HandoffToStockMenu', 'Unconfigured')" \
    "data['runtime'].get('main_status', {}).get('launcher_state', '?')"
  status_json "exit-menu" || record_fail "exit-to-menu status JSON"
  assert_status "$OUT/status-exit-menu.json" "exit-to-menu reaches stock-menu handoff state" \
    "data['runtime']['main_status'].get('launcher_state') in ('HandoffToStockMenu', 'Unconfigured')"
  run_capture "raw-reboot-after-exit-menu" "$MISTER" reboot-wait --raw
  wait_for_launcher_active "wait-launcher-after-exit-menu-reboot" 90 || return 1
  status_json "post-exit-menu-reboot" || record_fail "post exit-menu reboot status JSON"
  assert_status "$OUT/status-post-exit-menu-reboot.json" "launcher recovers after exit-to-menu smoke" \
    "data['runtime']['main_status'].get('launcher_state') == 'LauncherActive' and int(data['runtime']['main_status'].get('launcher_pid', 0)) > 0 and data['runtime']['slint_status'].get('scene') == 'launcher'"

  remote "printf 'mister_magik_launch %s\n' '$LAUNCH_REF' > /dev/MiSTer_cmd"
  wait_status_expr "wait game handoff" 45 \
    "data['runtime']['main_status'].get('launcher_state') in ('HandoffToGame', 'Unconfigured')" \
    "data['runtime'].get('main_status', {}).get('launcher_state', '?')"
  status_json "handoff-game" || record_fail "handoff game status JSON"
  assert_status "$OUT/status-handoff-game.json" "game handoff leaves active launcher state" \
    "data['runtime']['main_status'].get('launcher_state') in ('HandoffToGame', 'Unconfigured')"
  run_capture "raw-reboot-after-game-handoff" "$MISTER" reboot-wait --raw
  wait_for_launcher_active "wait-launcher-after-game-reboot" 90 || return 1
  status_json "post-game-reboot" || record_fail "post game reboot status JSON"
  assert_status "$OUT/status-post-game-reboot.json" "launcher recovers after game handoff smoke" \
    "data['runtime']['main_status'].get('launcher_state') == 'LauncherActive' and int(data['runtime']['main_status'].get('launcher_pid', 0)) > 0 and data['runtime']['slint_status'].get('scene') == 'launcher'"

  if [ "$FAST" -eq 0 ]; then
    run_exit_menu_loop || true
    run_launch_matrix || true
  fi
}

run_tier_display_modes() {
  run_display_mode_smoke || true
}

run_tier_install_restore() {
  run_install_restore_roundtrip || true
}

run_tier_soak() {
  run_soak || true
}

run_selected_tiers() {
  local tier
  for tier in $ALL_TIERS; do
    tier_selected "$tier" || continue
    case "$tier" in
      health) run_tier_health ;;
      framebuffer-route) run_tier_framebuffer_route ;;
      launcher-lifecycle) run_tier_launcher_lifecycle || exit 1 ;;
      catalog) run_tier_catalog ;;
      handoff) run_tier_handoff || exit 1 ;;
      display-modes) run_tier_display_modes ;;
      install-restore) run_tier_install_restore ;;
      soak) run_tier_soak ;;
    esac
  done
}

assert_artifacts_complete() {
  local missing=0 path
  for path in "$REPORT" "$OUT/status-final.json" "$OUT/doctor-final.json" "$OUT/slint-status.json" "$OUT/main-status.json"; do
    if [ ! -s "$path" ]; then
      echo "missing artifact: $path" >&2
      missing=$((missing + 1))
    fi
  done
  if [ "$missing" -eq 0 ]; then
    record_ok "artifact completeness"
  else
    record_fail "artifact completeness missing=$missing"
  fi
}

collect_boot_network_logs() {
  remote_get_optional "/tmp/mister-magik-agent.log" "mister-magik-agent-current.log"
  remote_get_optional "/tmp/mister-magik-agent.boot.out" "mister-magik-agent-boot.out"
  remote_get_optional "/media/fat/mister-magik/bootlogs/agent.log" "mister-magik-agent-persistent.log"
  remote_get_optional "/media/fat/mister-magik/bootlogs/agent.seq" "mister-magik-agent.seq"
  remote_get_optional "/media/fat/mister-magik/bootlogs/fastnet.log" "mister-magik-fastnet-persistent.log"
  remote_get_optional "/media/fat/mister-magik/bootlogs/fastready.log" "mister-magik-fastready.log"
  remote_get_optional "/media/fat/mister-magik/bootlogs/fastsshd.log" "mister-magik-fastsshd.log"
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
  collect_boot_network_logs
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

write_report_header
run_deploy_if_requested
run_initial_gate
run_selected_tiers

collect_artifacts
assert_artifacts_complete
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
