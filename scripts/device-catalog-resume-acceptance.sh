#!/usr/bin/env bash
# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

# Destructive device acceptance for durable first-build catalog resume.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
MISTER="${MISTER:-$ROOT/scripts/mister}"
source "$ROOT/scripts/lib/magik-layout.sh"
source "$ROOT/scripts/lib/mister-fifo-lib.sh"
source "$ROOT/scripts/lib/catalog-device-test-lib.sh"

DEPLOY=skip
LABEL="catalog-resume-$(date -u +%Y%m%dT%H%M%SZ)"
SELECTED_INDEX=17
SELF_TEST=0
TIMEOUT="${CATALOG_RESUME_TIMEOUT:-240}"

usage() {
  echo "usage: scripts/device-catalog-resume-acceptance.sh [--deploy-device|--skip-build] [--label LABEL] [--selected-index INDEX] [--self-test]"
}

field() {
  local row="$1" key="$2"
  printf '%s\n' "$row" | awk -F '\t' -v key="$key" '
    { for (i=1; i<=NF; i++) if ($i ~ ("^" key "=")) { sub("^" key "=", "", $i); print $i; exit } }
  '
}

resume_field() {
  field "$1" "$2"
}

normalise_inspector() {
  awk -F '\t' '
    $1 == "catalog_v3_summary_tsv" {
      for (i=2; i<=NF; i++) if ($i ~ /^(fingerprint|total_games|systems)=/) print $i
    }
    $1 == "catalog_v3_system_tsv" {
      id=""; games=""
      for (i=2; i<=NF; i++) {
        if ($i ~ /^system_id=/) { id=$i; sub(/^system_id=/, "", id) }
        if ($i ~ /^games=/) { games=$i; sub(/^games=/, "", games) }
      }
      if (id != "" && games != "") print "system=" id " games=" games
    }
  ' | LC_ALL=C sort
}

while [ "$#" -gt 0 ]; do
  case "$1" in
    --deploy-device) DEPLOY=device; shift ;;
    --skip-build) DEPLOY=skip; shift ;;
    --label) LABEL="${2:?--label needs a value}"; shift 2 ;;
    --selected-index) SELECTED_INDEX="${2:?--selected-index needs an integer}"; shift 2 ;;
    --self-test) SELF_TEST=1; shift ;;
    -h|--help) usage; exit 0 ;;
    *) echo "ERROR: unknown argument: $1" >&2; usage >&2; exit 2 ;;
  esac
done

if [ "$SELF_TEST" -eq 1 ]; then
  fixture=$'catalog_resume_tsv\tbuild_id=abc\tphase=target-committed\ttarget_ordinal=7\ttarget_count=20\tcommitted=1\treused=6\tinvalidated=0\treason=durable'
  [ "$(resume_field "$fixture" build_id)" = abc ]
  [ "$(resume_field "$fixture" target_ordinal)" = 7 ]
  inspect=$'catalog_v3_summary_tsv\tvalid=1\tsystems=2\ttotal_games=9\tfingerprint=deadbeef\ncatalog_v3_system_tsv\tsystem_id=snes\tgames=4\ncatalog_v3_system_tsv\tsystem_id=arcade\tgames=5'
  expected=$'fingerprint=deadbeef\nsystem=arcade games=5\nsystem=snes games=4\nsystems=2\ntotal_games=9'
  [ "$(printf '%s\n' "$inspect" | normalise_inspector)" = "$expected" ]
  catalog_device_test_self_test >/dev/null
  mister_fifo_self_test
  echo "device-catalog-resume-acceptance self-test ok"
  exit 0
fi

[[ "$SELECTED_INDEX" =~ ^[0-9]+$ ]] || { echo "--selected-index must be a non-negative integer" >&2; exit 2; }
[[ "$TIMEOUT" =~ ^[0-9]+$ ]] || { echo "CATALOG_RESUME_TIMEOUT must be an integer" >&2; exit 2; }

case "$DEPLOY" in
  device) "$ROOT/scripts/deploy-rust.sh" --device --ui-scope launcher ;;
  skip) ;;
esac

magik_layout_select dev
REMOTE_BIN="$MISTER_MAGIK_BIN"
REMOTE_APP="$MISTER_MAGIK_APP_DIR"
REMOTE_CATALOG="$REMOTE_APP/catalog-v3"
REMOTE_BOOTSTRAP="$REMOTE_APP/arcade-bootstrap.nav.lz4b"
REMOTE_ENV="$REMOTE_APP/launcher.env"
REMOTE_LOG="/tmp/mister-magik-slint.log"
REMOTE_EVENTS="/tmp/mister-magik/events.jsonl"
REMOTE_STATUS="/tmp/mister-magik/status.json"
REMOTE_MAIN_STATUS="/tmp/mister-magik/main-status.json"
REMOTE_RETURN_STATE="/tmp/mister-magik/launcher-return-state.json"
REMOTE_GATE="/tmp/mister-magik/catalog-resume-gate"
REMOTE_WATCHER="/tmp/mister-magik/catalog-resume-watcher.pid"
REMOTE_JOURNAL="$REMOTE_CATALOG/state/build-progress.sqlite3"
RUN_TOKEN="catalog-resume-$$-$(date +%s)"
REMOTE_BACKUP="$REMOTE_APP/.$RUN_TOKEN"
OUT="$ROOT/build/catalog-resume-acceptance/$LABEL"
mkdir -p "$OUT"
BASELINE="$OUT/baseline-inspect.tsv"
FINAL="$OUT/final-inspect.tsv"
BASELINE_NORMAL="$OUT/baseline-normalised.txt"
FINAL_NORMAL="$OUT/final-normalised.txt"
ENV_LOCAL="$(mktemp)"
HAD_ENV=0
ENV_BACKUP=""
CLEANUP_ACTIVE=1

fail() {
  echo "FAIL: $*" >&2
  exit 1
}

remote() {
  "$MISTER" run "$1"
}

wait_remote() {
  local label="$1" timeout="$2" command="$3" output status
  set +e
  output="$(remote "elapsed=0; while [ \"\$elapsed\" -lt '$timeout' ]; do if $command; then exit 0; fi; sleep 1; elapsed=\$((elapsed + 1)); done; echo 'timeout waiting for $label' >&2; tail -160 '$REMOTE_LOG' 2>/dev/null || true; ps w | grep -E 'MiSTer|mister-magik-fb' | grep -v grep || true; exit 124" 2>&1)"
  status=$?
  set -e
  if [ "$status" -ne 0 ]; then
    printf '%s\n' "$output" >&2
    fail "$label failed with status $status"
  fi
  echo "ok: $label"
}

main_command() {
  remote "$(mister_fifo_remote_command "$1" 8)"
}

write_test_env() {
  printf '%s\n' \
    'export MISTER_CATALOG_REFRESH=auto' \
    'export MISTER_LAUNCHER_START_SCREEN=arcade' \
    'export MISTER_LAUNCHER_START_SYSTEM=arcade' \
    "export MISTER_ARCADE_SELECTED_INDEX=$SELECTED_INDEX" \
    'export MISTER_LAUNCHER_AUTO_LAUNCH_SELECTED=1' \
    "export MISTER_LAUNCHER_AUTO_LAUNCH_GATE=$REMOTE_GATE" >"$ENV_LOCAL"
  "$MISTER" put "$ENV_LOCAL" "$REMOTE_ENV" >/dev/null
}

arm_checkpoint_gate() {
  remote "rm -f '$REMOTE_GATE' '$REMOTE_LOG' '$REMOTE_EVENTS'; ( elapsed=0; while [ \"\$elapsed\" -lt '$TIMEOUT' ]; do if grep -q 'catalog_resume_tsv.*phase=target-committed' '$REMOTE_LOG' 2>/dev/null; then : > '$REMOTE_GATE'; exit 0; fi; sleep 1; elapsed=\$((elapsed + 1)); done; exit 124 ) >/tmp/catalog-resume-watcher.log 2>&1 & echo \$! > '$REMOTE_WATCHER'"
}

latest_resume_line() {
  remote "grep 'catalog_resume_tsv' '$REMOTE_LOG' 2>/dev/null | tail -1"
}

cleanup() {
  local status=$? cleanup_failed=0
  trap - EXIT INT TERM
  rm -f "$ENV_LOCAL"
  if [ "$CLEANUP_ACTIVE" -eq 1 ]; then
    remote "rm -f /tmp/catalog-resume-watcher-cleanup-failed; if [ -s '$REMOTE_WATCHER' ]; then watcher=\$(cat '$REMOTE_WATCHER'); case \"\$watcher\" in *[!0-9]*|'') ;; *) kill \"\$watcher\" 2>/dev/null || true; wait \"\$watcher\" 2>/dev/null || true; if kill -0 \"\$watcher\" 2>/dev/null; then : > /tmp/catalog-resume-watcher-cleanup-failed; fi ;; esac; fi; rm -f '$REMOTE_GATE' '$REMOTE_WATCHER' /tmp/catalog-resume-watcher.log '$REMOTE_ENV'" >/dev/null 2>&1 || true
    if ! remote "test -p /dev/MiSTer_cmd" >/dev/null 2>&1 || ! main_command "load_core menu.rbf" >/dev/null 2>&1; then
      "$MISTER" reboot-wait --raw >/dev/null 2>&1 || true
    fi
    remote "killall mister-magik-fb 2>/dev/null || true; rm -rf '$REMOTE_CATALOG'; if [ -d '$REMOTE_BACKUP/catalog-v3' ]; then mv '$REMOTE_BACKUP/catalog-v3' '$REMOTE_CATALOG'; fi; rm -f '$REMOTE_BOOTSTRAP'; if [ -f '$REMOTE_BACKUP/arcade-bootstrap.nav.lz4b' ]; then mv '$REMOTE_BACKUP/arcade-bootstrap.nav.lz4b' '$REMOTE_BOOTSTRAP'; fi; if [ -f '$REMOTE_BACKUP/launcher.env' ]; then mv '$REMOTE_BACKUP/launcher.env' '$REMOTE_ENV'; fi; rmdir '$REMOTE_BACKUP' 2>/dev/null || true; rm -f '$REMOTE_GATE' '$REMOTE_WATCHER' /tmp/catalog-resume-watcher.log" >/dev/null 2>&1 || true
    main_command "mister_magik_restart_launcher" >/dev/null 2>&1 || true
    if ! remote "test ! -e '$REMOTE_GATE'; test ! -e '$REMOTE_WATCHER'; test ! -e /tmp/catalog-resume-watcher-cleanup-failed; test ! -e /tmp/mister-magik/fs-fault-launcher.env; test ! -e /tmp/mister-magik/fs-fault-session; test ! -e /tmp/mister-magik/fs-fault.json; test ! -e /media/fat/mister-magik/rebuild-on-next-boot; test ! -e /media/fat/mister-magik-dev/rebuild-on-next-boot; ! grep -q 'MISTER_LAUNCHER_AUTO_LAUNCH_SELECTED=1' '$REMOTE_ENV' 2>/dev/null"; then
      echo "FAIL: cleanup left a test arming file" >&2
      cleanup_failed=1
    fi
  fi
  if [ "$cleanup_failed" -ne 0 ] && [ "$status" -eq 0 ]; then status=1; fi
  exit "$status"
}
trap cleanup EXIT INT TERM

echo "==> Capturing valid V3 baseline"
remote "test -x '$REMOTE_BIN'; test -d '$REMOTE_CATALOG'; '$REMOTE_BIN' catalog-v3-inspect" >"$BASELINE"
grep -q $'catalog_v3_summary_tsv\tvalid=1' "$BASELINE" || fail "baseline catalog is not valid"
normalise_inspector <"$BASELINE" >"$BASELINE_NORMAL"

echo "==> Backing up catalog, bootstrap index, and launcher environment"
remote "rm -rf '$REMOTE_BACKUP'; mkdir -p '$REMOTE_BACKUP'; cp -a '$REMOTE_CATALOG' '$REMOTE_BACKUP/catalog-v3'; if [ -f '$REMOTE_BOOTSTRAP' ]; then cp -p '$REMOTE_BOOTSTRAP' '$REMOTE_BACKUP/arcade-bootstrap.nav.lz4b'; fi; if [ -f '$REMOTE_ENV' ]; then cp -p '$REMOTE_ENV' '$REMOTE_BACKUP/launcher.env'; fi"

echo "==> Creating genuine cold first build"
remote "rm -rf '$REMOTE_CATALOG'; rm -f '$REMOTE_BOOTSTRAP' '$REMOTE_GATE' '$REMOTE_LOG' '$REMOTE_EVENTS'"
write_test_env
arm_checkpoint_gate
main_command "mister_magik_restart_launcher"
wait_remote "initial cold launcher" 45 "ps w | grep -q '[m]ister-magik-fb ui launcher'"
ACTIVE_PID="$(remote "ps w | awk '/[m]ister-magik-fb ui launcher/ { print \$1; exit }'")"

BUILD_ID=""
PREVIOUS_CHECKPOINTS=0
HANDOFFS=0
PIDS=""

for cycle in 1 2 3; do
  launcher_pid="$ACTIVE_PID"
  [[ "$launcher_pid" =~ ^[0-9]+$ ]] || fail "cycle $cycle launcher PID missing"
  wait_remote "cycle $cycle durable target gate" "$TIMEOUT" "test -f '$REMOTE_GATE'"
  commit_line="$(remote "grep 'catalog_resume_tsv.*phase=target-committed' '$REMOTE_LOG' | tail -1")"
  ordinal="$(resume_field "$commit_line" target_ordinal)"
  checkpoint_count="$(resume_field "$commit_line" committed)"
  [[ "$ordinal" =~ ^[0-9]+$ ]] || fail "cycle $cycle missing committed target ordinal"
  [[ "$checkpoint_count" =~ ^[0-9]+$ ]] || fail "cycle $cycle missing durable checkpoint count"
  [ "$checkpoint_count" -gt "$PREVIOUS_CHECKPOINTS" ] || fail "durable checkpoint count did not increase ($PREVIOUS_CHECKPOINTS -> $checkpoint_count)"
  reused_count="$(resume_field "$commit_line" reused)"
  [ "${reused_count:-0}" -ge "$PREVIOUS_CHECKPOINTS" ] || fail "cycle $cycle did not reuse all $PREVIOUS_CHECKPOINTS prior checkpoints"
  PREVIOUS_CHECKPOINTS="$checkpoint_count"
  cycle_id="$(resume_field "$commit_line" build_id)"
  [ -n "$cycle_id" ] || fail "cycle $cycle missing build ID"
  if [ -z "$BUILD_ID" ]; then BUILD_ID="$cycle_id"; else [ "$cycle_id" = "$BUILD_ID" ] || fail "build ID changed"; fi

  PIDS="$PIDS $launcher_pid"
  wait_remote "cycle $cycle real game handoff" 45 "! kill -0 '$launcher_pid' 2>/dev/null"
  wait_remote "cycle $cycle Main-mediated game state" 15 "test -s '$REMOTE_RETURN_STATE' && { grep -q '\"launcher_state\":\"HandoffToGame\"' '$REMOTE_MAIN_STATUS' 2>/dev/null || grep -q '\"launcher_state\":\"Unconfigured\"' '$REMOTE_MAIN_STATUS' 2>/dev/null; }"
  HANDOFFS=$((HANDOFFS + 1))

  if [ "$cycle" -lt 3 ]; then
    write_test_env
    arm_checkpoint_gate
    main_command "load_core menu.rbf"
    wait_remote "cycle $cycle resumed launcher" 45 "ps w | grep -q '[m]ister-magik-fb ui launcher'"
    ACTIVE_PID="$(remote "ps w | awk '/[m]ister-magik-fb ui launcher/ { print \$1; exit }'")"
    wait_remote "cycle $cycle resume evidence" "$TIMEOUT" "grep -q 'catalog_resume_tsv.*phase=target-reused' '$REMOTE_LOG' 2>/dev/null"
    resumed_id="$(remote "grep 'catalog_resume_tsv.*phase=target-reused' '$REMOTE_LOG' | tail -1" | awk -F '\t' '{for(i=1;i<=NF;i++) if($i ~ /^build_id=/){sub(/^build_id=/,"",$i); print $i; exit}}')"
    [ "$resumed_id" = "$BUILD_ID" ] || fail "cycle $cycle resumed with different build ID"
  else
    remote "rm -f '$REMOTE_ENV' '$REMOTE_GATE' '$REMOTE_WATCHER' /tmp/catalog-resume-watcher.log"
    main_command "load_core menu.rbf"
    wait_remote "cycle 3 resumed launcher" 45 "ps w | grep -q '[m]ister-magik-fb ui launcher'"
    wait_remote "cycle 3 resume evidence" "$TIMEOUT" "grep -q 'catalog_resume_tsv.*phase=target-reused' '$REMOTE_LOG' 2>/dev/null"
    final_resume_line="$(remote "grep 'catalog_resume_tsv.*phase=target-reused' '$REMOTE_LOG' | tail -1")"
    [ "$(resume_field "$final_resume_line" build_id)" = "$BUILD_ID" ] || fail "cycle 3 resumed with different build ID"
    [ "$(resume_field "$final_resume_line" reused)" -ge "$PREVIOUS_CHECKPOINTS" ] || fail "cycle 3 did not reuse all prior checkpoints"
  fi
done

[ "$HANDOFFS" -eq 3 ] || fail "expected exactly three handoffs"
[ "$(printf '%s\n' "$PIDS" | awk '{print NF}')" -eq 3 ] || fail "expected three launcher PIDs"
[ "$(printf '%s\n' "$PIDS" | tr ' ' '\n' | awk 'NF && !seen[$0]++ {count++} END {print count+0}')" -eq 3 ] || fail "launcher PIDs were not distinct"
wait_remote "catalog Persisted" "$TIMEOUT" "grep -q 'catalog_builder_event_tsv.*event=Persisted' '$REMOTE_LOG' 2>/dev/null"
wait_remote "catalog Done" 45 "grep -q 'catalog_builder_event_tsv.*event=Done' '$REMOTE_LOG' 2>/dev/null"
remote "! grep -qi 'persist.*fail' '$REMOTE_LOG'; test ! -e '$REMOTE_JOURNAL'; test -z \"\$(find '$REMOTE_CATALOG' \( -name '*.stage.*' -o -name '*.tmp.*' -o -name 'reconcile-*' \) 2>/dev/null)\"; test -z \"\$(find /tmp/mister-magik/catalog-v3-build -mindepth 1 2>/dev/null)\"; test \"\$(ps w | grep '[m]ister-magik-fb ui launcher' | wc -l)\" = 1" || fail "final durability cleanup or launcher count"

remote "'$REMOTE_BIN' catalog-v3-inspect" >"$FINAL"
grep -q $'catalog_v3_summary_tsv\tvalid=1' "$FINAL" || fail "final catalog inspector failed"
normalise_inspector <"$FINAL" >"$FINAL_NORMAL"
cmp -s "$BASELINE_NORMAL" "$FINAL_NORMAL" || {
  diff -u "$BASELINE_NORMAL" "$FINAL_NORMAL" >&2 || true
  fail "final fingerprint, totals, or per-system counts differ from baseline"
}

echo "catalog_resume_acceptance_tsv\tlabel=$LABEL\tbuild_id=$BUILD_ID\thandoffs=$HANDOFFS\tdurable_checkpoints=$PREVIOUS_CHECKPOINTS\tresult=pass"
