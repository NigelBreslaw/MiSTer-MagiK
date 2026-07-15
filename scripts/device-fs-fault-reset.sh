#!/usr/bin/env bash
# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

# Destructively reset-fault MagiK filesystem writes on a real MiSTer.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
MISTER="${MISTER:-$ROOT/scripts/mister}"
REMOTE_BIN="/media/fat/mister-magik-dev/mister-magik-fb"
REMOTE_DIR="/media/fat/mister-magik-dev"
REMOTE_ENV="$REMOTE_DIR/launcher.env"
REMOTE_DB="$REMOTE_DIR/library.sqlite3"
REMOTE_SUMMARY="$REMOTE_DIR/library.summary.json"
REMOTE_NAV="$REMOTE_DIR/library.nav.lz4b"
REMOTE_ASSETS="$REMOTE_DIR/assets"
REMOTE_STATE="$REMOTE_ASSETS/.screenshot-media-state.json"
REMOTE_MARKER="/tmp/mister-magik/fs-fault.json"
REMOTE_SESSION="/tmp/mister-magik/fs-fault-session"
REMOTE_FAULT_ENV="/tmp/mister-magik/fs-fault-launcher.env"
PUBLIC_ENV="/media/fat/mister-magik/launcher.env"
PUBLIC_REBUILD_MARKER="/media/fat/mister-magik/rebuild-on-next-boot"
REMOTE_REBUILD_MARKER="$REMOTE_DIR/rebuild-on-next-boot"
REMOTE_LOCK="/tmp/mister-magik/library-refresh.lock"
REMOTE_LOG="/tmp/mister-magik-fs-fault-refresh.log"
TSV="$ROOT/history/toolchain-bench/results-fs-fault-reset.tsv"

LABEL=""
SCENARIO="all"
ITERATIONS=1
WAIT_TIMEOUT=40
MAX_WAIT_TIMEOUT=40
SETTLE=5
RUN_ACCEPTANCE=1
RECOVER_ONLY=0
ACTIVE_TRIGGER_PID=""
ACTIVE_FAULT_SESSION=""
TRIGGER_LABEL=""

usage() {
  cat <<'EOF'
usage: scripts/device-fs-fault-reset.sh LABEL [--scenario NAME] [--iterations N] [--wait-timeout SECS] [--settle SECS] [--no-acceptance] [--recover-only]

Scenarios: catalog, projections, media, settings-marker, reset-delete, all

This is intentionally destructive. It removes MagiK catalog DB/projection files
and screenshot media artifacts, then rebuilds/redownloads as needed.

--recover-only clears stale fault launcher env and disposable artifacts, rebuilds
the catalog, restarts the launcher, and exits.

--wait-timeout is capped at 40 seconds. Reset-fault points fail fast when a
reset is not observed.
EOF
}

while [ "$#" -gt 0 ]; do
  case "$1" in
    --scenario) SCENARIO="${2:?--scenario needs a value}"; shift 2 ;;
    --iterations) ITERATIONS="${2:?--iterations needs a value}"; shift 2 ;;
    --wait-timeout) WAIT_TIMEOUT="${2:?--wait-timeout needs seconds}"; shift 2 ;;
    --settle) SETTLE="${2:?--settle needs seconds}"; shift 2 ;;
    --no-acceptance) RUN_ACCEPTANCE=0; shift ;;
    --recover-only) RECOVER_ONLY=1; shift ;;
    -h|--help) usage; exit 0 ;;
    --*) echo "unknown option: $1" >&2; usage >&2; exit 2 ;;
    *)
      if [ -n "$LABEL" ]; then
        echo "unexpected argument: $1" >&2
        usage >&2
        exit 2
      fi
      LABEL="$1"
      shift
      ;;
  esac
done

if [ -z "$LABEL" ]; then
  LABEL="FSFAULT-$(date -u +%Y%m%dT%H%M%SZ)"
fi
if [[ ! "$LABEL" =~ ^[A-Za-z0-9_.-]+$ ]]; then
  echo "label must contain only letters, numbers, _, ., or -" >&2
  exit 2
fi
if [[ ! "$ITERATIONS" =~ ^[0-9]+$ || "$ITERATIONS" -lt 1 ]]; then
  echo "--iterations must be a positive integer" >&2
  exit 2
fi
if [[ ! "$WAIT_TIMEOUT" =~ ^[0-9]+$ || "$WAIT_TIMEOUT" -lt 1 ]]; then
  echo "--wait-timeout must be a positive integer" >&2
  exit 2
fi
if [ "$WAIT_TIMEOUT" -gt "$MAX_WAIT_TIMEOUT" ]; then
  echo "--wait-timeout must be <= $MAX_WAIT_TIMEOUT seconds" >&2
  exit 2
fi

mkdir -p "$(dirname "$TSV")"
if [ ! -f "$TSV" ]; then
  printf 'label\tcommit\tscenario\titeration\tfault_point\ttrigger\tdown_seen\tlauncher_ready\tdb_ok\tmedia_state_ok\tacceptance_ok\tresult\tnotes\n' >"$TSV"
fi

remote() {
  "$MISTER" run "$1"
}

remote_quick() {
  if command -v perl >/dev/null 2>&1; then
    perl -e 'alarm shift; exec @ARGV' 5 "$MISTER" run "$1"
  else
    "$MISTER" run "$1"
  fi
}

sq() {
  printf "'%s'" "$(printf "%s" "$1" | sed "s/'/'\\\\''/g")"
}

record_row() {
  local scenario="$1" iteration="$2" point="$3" trigger="$4" down_seen="$5" launcher_ready="$6" db_ok="$7" media_state_ok="$8" acceptance_ok="$9" result="${10}" notes="${11}"
  local commit
  commit="$(git -C "$ROOT" rev-parse --short HEAD 2>/dev/null || echo unknown)"
  printf '%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\n' \
    "$LABEL" "$commit" "$scenario" "$iteration" "$point" "$trigger" "$down_seen" \
    "$launcher_ready" "$db_ok" "$media_state_ok" "$acceptance_ok" "$result" "$notes" >>"$TSV"
}

points_for_scenario() {
  case "$1" in
    catalog)
      printf '%s\n' \
        catalog.sqlite.after_build_temp_sync \
        catalog.sqlite.after_final_temp_copy \
        catalog.sqlite.after_final_temp_sync \
        catalog.sqlite.after_rename_before_parent_sync
      ;;
    projections)
      printf '%s\n' \
        catalog.summary.after_temp_write \
        catalog.summary.after_temp_sync \
        catalog.summary.after_rename_before_parent_sync \
        catalog.navigation.after_temp_write \
        catalog.navigation.after_temp_sync \
        catalog.navigation.after_rename_before_parent_sync
      ;;
    media)
      printf '%s\n' \
        media.pack.after_temp_write \
        media.pack.after_temp_sync \
        media.pack.after_rename_before_parent_sync \
        media.index.after_temp_write \
        media.index.after_temp_sync \
        media.index.after_rename_before_parent_sync \
        media.state.after_temp_write \
        media.state.after_temp_sync \
        media.state.after_rename_before_parent_sync
      ;;
    settings-marker)
      printf '%s\n' \
        settings.after_temp_write \
        settings.after_rename \
        launcher.rebuild_marker.after_write
      ;;
    reset-delete)
      printf '%s\n' \
        reset_delete.database.after_remove \
        reset_delete.summary.after_remove \
        reset_delete.navigation.after_remove \
        reset_delete.screenshot_asset.after_remove
      ;;
    *) return 1 ;;
  esac
}

scenarios_to_run() {
  if [ "$SCENARIO" = "all" ]; then
    printf '%s\n' catalog projections media settings-marker reset-delete
  else
    points_for_scenario "$SCENARIO" >/dev/null || {
      echo "unknown scenario: $SCENARIO" >&2
      exit 2
    }
    printf '%s\n' "$SCENARIO"
  fi
}

cleanup_destructive_state() {
  remote "rm -f $(sq "$REMOTE_ENV") $(sq "$REMOTE_FAULT_ENV") $(sq "$REMOTE_MARKER") $(sq "$REMOTE_SESSION") $(sq "$REMOTE_LOCK") $(sq "$REMOTE_DB") $(sq "$REMOTE_SUMMARY") $(sq "$REMOTE_NAV") $(sq "$REMOTE_DIR/.library.sqlite3.tmp")* $(sq "$REMOTE_DIR/.library.summary.json.tmp")* $(sq "$REMOTE_DIR/.library.nav.lz4b.tmp")* $(sq "$REMOTE_DIR/rebuild-on-next-boot"); mkdir -p $(sq "$REMOTE_ASSETS"); find $(sq "$REMOTE_ASSETS") -maxdepth 1 -type f \\( -name '*-screenshots*.mmlz4b' -o -name '*-screenshots*.mmlz4b.idx' -o -name '.*-screenshots*.mmlz4b.tmp*' -o -name '.screenshot-media-state.json*' \\) -delete; rm -rf /tmp/mister-magik-media-download; sync" >/dev/null
}

recover_catalog_and_launcher() {
  remote "$(sq "$REMOTE_BIN") library-refresh >$(sq "$REMOTE_LOG") 2>&1" >/dev/null || {
    echo "library-refresh recovery failed; log follows" >&2
    remote "tail -160 $(sq "$REMOTE_LOG") 2>/dev/null || true" >&2 || true
    return 1
  }
  remote "if [ -p /dev/MiSTer_cmd ]; then printf 'mister_magik_restart_launcher\n' > /dev/MiSTer_cmd; fi" >/dev/null || true
}

wait_for_down_up() {
  local down_seen=0 deadline
  deadline=$((SECONDS + WAIT_TIMEOUT))
  while [ "$SECONDS" -lt "$deadline" ]; do
    if ! remote_quick ":" >/dev/null 2>&1; then
      down_seen=1
      break
    fi
    sleep 1
  done
  if [ "$down_seen" = "1" ]; then
    "$MISTER" wait "$WAIT_TIMEOUT" >/dev/null
  fi
  echo "$down_seen"
}

launcher_ready() {
  remote "test \"\$(ps w | grep '[m]ister-magik-fb ui launcher' | wc -l)\" = 1" >/dev/null 2>&1
}

db_ok() {
  "$MISTER" catalog counts >/dev/null 2>&1
}

media_state_ok() {
  remote "test ! -f $(sq "$REMOTE_STATE") || grep -q '\"schema\"' $(sq "$REMOTE_STATE")" >/dev/null 2>&1
}

run_acceptance() {
  if [ "$RUN_ACCEPTANCE" -eq 0 ]; then
    return 0
  fi
  "$ROOT/scripts/device-catalog-acceptance.sh" --settle "$SETTLE" >/tmp/mister-magik-fs-fault-acceptance.log 2>&1
}

remote_command_available() {
  local command="$1"
  remote "$(sq "$REMOTE_BIN") $(sq "$command") --help >/dev/null 2>&1" >/dev/null 2>&1
}

preflight_scenario() {
  local scenario="$1"
  case "$scenario" in
    media)
      remote_command_available media-bench-save || {
        echo "ERROR: deployed binary does not expose media-bench-save; deploy a --bench-tools build before media fault tests" >&2
        return 1
      }
      ;;
    reset-delete)
      remote_command_available reset-delete-database || {
        echo "ERROR: deployed binary does not expose reset-delete-database; deploy the current MagiK build before reset-delete fault tests" >&2
        return 1
      }
      remote_command_available reset-delete-screenshot-packs || {
        echo "ERROR: deployed binary does not expose reset-delete-screenshot-packs; deploy the current MagiK build before reset-delete fault tests" >&2
        return 1
      }
      ;;
  esac
}

fault_env_prefix() {
  local point="$1"
  if [ -z "$ACTIVE_FAULT_SESSION" ]; then
    echo "internal error: fault session is not armed" >&2
    exit 2
  fi
  printf 'MISTER_FS_FAULT_POINT=%s MISTER_FS_FAULT_ACTION=direct-reset-no-sync MISTER_FS_FAULT_DELAY_MS=2000 MISTER_FS_FAULT_SESSION=%s ' "$(sq "$point")" "$(sq "$ACTIVE_FAULT_SESSION")"
}

arm_fault_session() {
  local point="$1" iteration="${2:-0}"
  ACTIVE_FAULT_SESSION="${LABEL}:${point}:${iteration}:$$:${RANDOM}"
  remote "mkdir -p /tmp/mister-magik; printf %s $(sq "$ACTIVE_FAULT_SESSION") >$(sq "$REMOTE_SESSION")" >/dev/null
}

trigger_catalog_refresh() {
  local point="$1" iteration="$2"
  arm_fault_session "$point" "$iteration"
  remote "$(fault_env_prefix "$point") MISTER_LIBRARY_BENCH_LABEL=$(sq "$LABEL") MISTER_LIBRARY_BENCH_ACTIVE_ITERATION=$(sq "$iteration") $(sq "$REMOTE_BIN") library-refresh" >/tmp/mister-magik-fs-fault-trigger.log 2>&1 &
  ACTIVE_TRIGGER_PID=$!
}

trigger_rebuild_marker() {
  local point="$1"
  arm_fault_session "$point"
  remote "$(fault_env_prefix "$point") $(sq "$REMOTE_BIN") request-library-rebuild" >/tmp/mister-magik-fs-fault-trigger.log 2>&1 &
  ACTIVE_TRIGGER_PID=$!
}

trigger_reset_delete_database() {
  local point="$1"
  arm_fault_session "$point"
  remote "$(fault_env_prefix "$point") $(sq "$REMOTE_BIN") reset-delete-database" >/tmp/mister-magik-fs-fault-trigger.log 2>&1 &
  ACTIVE_TRIGGER_PID=$!
}

trigger_reset_delete_screenshot_packs() {
  local point="$1"
  arm_fault_session "$point"
  remote "mkdir -p $(sq "$REMOTE_ASSETS"); printf dummy >$(sq "$REMOTE_ASSETS/arcade-screenshots-320x320.mmlz4b"); sync; $(fault_env_prefix "$point") $(sq "$REMOTE_BIN") reset-delete-screenshot-packs" >/tmp/mister-magik-fs-fault-trigger.log 2>&1 &
  ACTIVE_TRIGGER_PID=$!
}

trigger_settings_toggle() {
  local point="$1"
  arm_fault_session "$point"
  remote "$(fault_env_prefix "$point") $(sq "$REMOTE_BIN") toggle-simple-joystick-setting" >/tmp/mister-magik-fs-fault-trigger.log 2>&1 &
  ACTIVE_TRIGGER_PID=$!
}

trigger_media_pack_bench() {
  local point="$1" iteration="$2" artifact="${3:-pack}"
  arm_fault_session "$point" "$iteration"
  remote "$(fault_env_prefix "$point") $(sq "$REMOTE_BIN") media-bench-save --label $(sq "$LABEL-$iteration") --system arcade --iterations 1 --size-bytes 1048576 --artifact $(sq "$artifact")" >/tmp/mister-magik-fs-fault-trigger.log 2>&1 &
  ACTIVE_TRIGGER_PID=$!
}

trigger_launcher_with_env() {
  local point="$1" input_script="${2:-}"
  arm_fault_session "$point"
  local args=(
    launcher-restart
    --remote-env "$REMOTE_FAULT_ENV"
    --env "MISTER_FS_FAULT_POINT=$point"
    --env "MISTER_FS_FAULT_ACTION=direct-reset-no-sync"
    --env "MISTER_FS_FAULT_DELAY_MS=2000"
    --env "MISTER_FS_FAULT_SESSION=$ACTIVE_FAULT_SESSION"
    --env "MISTER_CATALOG_BACKGROUND_DELAY_MS=0"
    --timeout 20
  )
  if [ -n "$input_script" ]; then
    args+=(--env "MISTER_LAUNCHER_INPUT_SCRIPT=$input_script")
  fi
  "$MISTER" "${args[@]}" >/tmp/mister-magik-fs-fault-trigger.log 2>&1 &
  ACTIVE_TRIGGER_PID=$!
}

stop_active_trigger() {
  if [ -n "${ACTIVE_TRIGGER_PID:-}" ]; then
    kill "$ACTIVE_TRIGGER_PID" >/dev/null 2>&1 || true
    wait "$ACTIVE_TRIGGER_PID" >/dev/null 2>&1 || true
    ACTIVE_TRIGGER_PID=""
  fi
}

cleanup_on_exit() {
  stop_active_trigger
  remote_quick "rm -f $(sq "$REMOTE_ENV") $(sq "$PUBLIC_ENV") $(sq "$REMOTE_FAULT_ENV") $(sq "$REMOTE_MARKER") $(sq "$REMOTE_SESSION") $(sq "$REMOTE_REBUILD_MARKER") $(sq "$PUBLIC_REBUILD_MARKER")" >/dev/null 2>&1 || true
}

trap cleanup_on_exit EXIT INT TERM

trigger_point() {
  local scenario="$1" point="$2" iteration="$3"
  ACTIVE_TRIGGER_PID=""
  case "$scenario:$point" in
    media:media.pack.*) trigger_media_pack_bench "$point" "$iteration"; TRIGGER_LABEL=media-bench-save ;;
    media:media.index.*) trigger_media_pack_bench "$point" "$iteration" "index"; TRIGGER_LABEL=media-bench-save-index ;;
    media:media.state.*) trigger_media_pack_bench "$point" "$iteration" "state"; TRIGGER_LABEL=media-bench-save-state ;;
    settings-marker:settings.*) trigger_settings_toggle "$point"; TRIGGER_LABEL=toggle-simple-joystick-setting ;;
    settings-marker:launcher.rebuild_marker.after_write) trigger_rebuild_marker "$point"; TRIGGER_LABEL=request-library-rebuild ;;
    reset-delete:reset_delete.screenshot_asset.after_remove) cleanup_destructive_state; recover_catalog_and_launcher >/dev/null || true; trigger_reset_delete_screenshot_packs "$point"; TRIGGER_LABEL=reset-delete-screenshot-packs ;;
    reset-delete:*) cleanup_destructive_state; recover_catalog_and_launcher >/dev/null || true; trigger_reset_delete_database "$point"; TRIGGER_LABEL=reset-delete-database ;;
    *) trigger_catalog_refresh "$point" "$iteration"; TRIGGER_LABEL=library-refresh ;;
  esac
}

if [ "$RECOVER_ONLY" -eq 1 ]; then
  cleanup_destructive_state
  recover_catalog_and_launcher
  echo "fs fault recovery completed"
  exit 0
fi

run_one() {
  local scenario="$1" point="$2" iteration="$3"
  echo "==> scenario=$scenario iteration=$iteration fault=$point"
  cleanup_destructive_state
  recover_catalog_and_launcher >/dev/null || true

  local trigger down_seen marker_seen launcher_state db_state media_state acceptance_state result notes
  trigger_point "$scenario" "$point" "$iteration"
  trigger="$TRIGGER_LABEL"
  down_seen="$(wait_for_down_up || echo 0)"
  marker_seen=0
  remote_quick "test -f $(sq "$REMOTE_MARKER")" >/dev/null 2>&1 && marker_seen=1 || true
  stop_active_trigger

  launcher_state=0
  db_state=0
  media_state=0
  acceptance_state=0
  result=fail
  notes=""

  if [ "$down_seen" != "1" ]; then
    if [ "$marker_seen" = "1" ]; then
      notes="${notes}fault_marker_without_reset;"
    else
      notes="${notes}fault_not_observed;"
    fi
  fi

  if recover_catalog_and_launcher; then
    sleep "$SETTLE"
    launcher_ready && launcher_state=1 || notes="${notes}launcher_not_ready;"
    db_ok && db_state=1 || notes="${notes}db_query_failed;"
    media_state_ok && media_state=1 || notes="${notes}media_state_bad;"
    if run_acceptance; then
      acceptance_state=1
    else
      notes="${notes}acceptance_failed;"
    fi
  else
    notes="${notes}recovery_failed;"
  fi

  if [ "$down_seen" = "1" ] && [ "$launcher_state" = "1" ] && [ "$db_state" = "1" ] && [ "$media_state" = "1" ] && [ "$acceptance_state" = "1" ]; then
    result=ok
  fi
  record_row "$scenario" "$iteration" "$point" "$trigger" "$down_seen" "$launcher_state" "$db_state" "$media_state" "$acceptance_state" "$result" "${notes:-ok}"
  if [ "$result" != "ok" ]; then
    echo "WARN: scenario=$scenario fault=$point result=$result notes=${notes:-none}" >&2
    return 1
  fi
}

for scenario in $(scenarios_to_run); do
  preflight_scenario "$scenario"
  while IFS= read -r point; do
    [ -n "$point" ] || continue
    for ((iteration = 1; iteration <= ITERATIONS; iteration++)); do
      run_one "$scenario" "$point" "$iteration"
    done
  done < <(points_for_scenario "$scenario")
done

cleanup_destructive_state
recover_catalog_and_launcher >/dev/null || true
echo "fs fault results appended to $TSV"
