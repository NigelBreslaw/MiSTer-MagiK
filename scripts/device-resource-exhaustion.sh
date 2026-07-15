#!/usr/bin/env bash
# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

# Attended MiSTer resource-exhaustion repros for MagiK launcher robustness.
#
# The script never edits boot config or arms reboot/fault injection. It creates
# only volatile test pressure, captures artifacts, cleans up, and restarts the
# supervised launcher before exiting.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
MISTER="$ROOT/scripts/mister"
RUN_RUST="$ROOT/scripts/run-rust.sh"

SCENARIO="tmp-enospc"
EXPECT="survive"
LABEL=""
POLL_SECS="${MISTER_RESOURCE_EXHAUSTION_POLL_SECS:-30}"

usage() {
  cat <<'EOF'
usage: scripts/device-resource-exhaustion.sh [--scenario tmp-enospc|memory-pressure] [--expect crash|survive] [--label LABEL]

Attended MiSTer device repros for launcher resource exhaustion.

Defaults:
  --scenario tmp-enospc
  --expect survive

Environment:
  MISTER_RESOURCE_EXHAUSTION_POLL_SECS       poll window, default 30
  MISTER_RESOURCE_MEMORY_TARGET_MIB          memory-pressure target, default 360
  MISTER_RESOURCE_MEMORY_CHUNK_MIB           memory-pressure chunk size, default 8
EOF
}

while [ "$#" -gt 0 ]; do
  case "$1" in
    --scenario)
      SCENARIO="${2:-}"
      shift 2
      ;;
    --expect)
      EXPECT="${2:-}"
      shift 2
      ;;
    --label)
      LABEL="${2:-}"
      shift 2
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      echo "unknown argument: $1" >&2
      usage >&2
      exit 2
      ;;
  esac
done

case "$SCENARIO" in
  tmp-enospc|memory-pressure) ;;
  *)
    echo "unknown scenario: $SCENARIO" >&2
    exit 2
    ;;
esac

case "$EXPECT" in
  crash|survive) ;;
  *)
    echo "unknown expectation: $EXPECT" >&2
    exit 2
    ;;
esac

if [ -z "$LABEL" ]; then
  LABEL="resource-${SCENARIO}-${EXPECT}-$(date -u +%Y%m%dT%H%M%SZ)"
fi

OUT="$ROOT/build/resource-exhaustion/$LABEL"
REMOTE_FILL_DIR="/tmp/mister-magik-resource-exhaustion"
REMOTE_FILL_FILE="$REMOTE_FILL_DIR/fill.bin"
REMOTE_DD_LOG="/media/fat/mister-magik/resource-exhaustion-dd.log"
REMOTE_MEM_SCRIPT="$REMOTE_FILL_DIR/memory-pressure.py"
REMOTE_MEM_PID="$REMOTE_FILL_DIR/memory-pressure.pid"
REMOTE_MEM_LOG="/media/fat/mister-magik/resource-exhaustion-memory.log"
REMOTE_LOG="/tmp/mister-magik-slint.log"
REMOTE_EVENTS="/tmp/mister-magik/events.jsonl"
REMOTE_STATUS="/tmp/mister-magik/status.json"
REMOTE_MAIN_STATUS="/tmp/mister-magik/main-status.json"
REMOTE_CRASH_DIR="/media/fat/mister-magik/crashes"
REMOTE_BIN="/media/fat/mister-magik/mister-magik-fb"
MEMORY_TARGET_MIB="${MISTER_RESOURCE_MEMORY_TARGET_MIB:-360}"
MEMORY_CHUNK_MIB="${MISTER_RESOURCE_MEMORY_CHUNK_MIB:-8}"

mkdir -p "$OUT"

remote() {
  "$MISTER" run "$1"
}

launcher_pid() {
  remote "pidof mister-magik-fb 2>/dev/null || true" 2>/dev/null | awk 'NF { print $1; exit }'
}

launcher_running() {
  remote "pidof mister-magik-fb >/dev/null 2>&1" >/dev/null 2>&1
}

latest_crash_report() {
  remote "ls -1t '$REMOTE_CRASH_DIR'/report-*.json 2>/dev/null | sed -n '1p'" 2>/dev/null || true
}

main_reports_launcher_crashed() {
  remote "grep -q '\"launcher_state\":\"LauncherCrashed\"' '$REMOTE_MAIN_STATUS' 2>/dev/null" >/dev/null 2>&1
}

cleanup_remote() {
  echo "==> Cleaning resource-exhaustion repro files"
  remote "if [ -s '$REMOTE_MEM_PID' ]; then kill -9 \$(cat '$REMOTE_MEM_PID') 2>/dev/null || true; fi; rm -rf '$REMOTE_FILL_DIR'; rm -f '$REMOTE_DD_LOG' '$REMOTE_MEM_LOG'; df -h /tmp; if [ -p /dev/MiSTer_cmd ]; then printf 'mister_magik_restart_launcher\n' > /dev/MiSTer_cmd; fi" || true
}

dump_artifacts() {
  echo "==> Capturing repro artifacts in $OUT"
  remote "echo MAIN_STATUS; cat '$REMOTE_MAIN_STATUS' 2>/dev/null || true; echo STATUS; cat '$REMOTE_STATUS' 2>/dev/null || true; echo PROCS; pidof mister-magik-fb 2>/dev/null || true; pidof MiSTer_MagiK 2>/dev/null || true; ps w | grep -E 'MiSTer|mister-magik-fb|resource-exhaustion' | grep -v grep || true; echo TMP; df -h /tmp; echo MEMINFO; sed -n '1,12p' /proc/meminfo; echo DD_LOG; cat '$REMOTE_DD_LOG' 2>/dev/null || true; echo MEMORY_LOG; cat '$REMOTE_MEM_LOG' 2>/dev/null || true; echo SLINT_LOG_TAIL; tail -160 '$REMOTE_LOG' 2>/dev/null || true; echo EVENTS_TAIL; tail -120 '$REMOTE_EVENTS' 2>/dev/null || true; echo CRASHES; ls -lt '$REMOTE_CRASH_DIR' 2>/dev/null | sed -n '1,16p' || true" >"$OUT/remote-dump.txt" || true
}

wait_for_launcher() {
  local elapsed=0
  while [ "$elapsed" -lt 60 ]; do
    if launcher_running; then
      return 0
    fi
    sleep 1
    elapsed=$((elapsed + 1))
  done
  return 1
}

start_clean_launcher() {
  echo "==> Starting clean supervised launcher"
  remote "rm -rf '$REMOTE_FILL_DIR'; rm -f '$REMOTE_DD_LOG' '$REMOTE_MEM_LOG' '$REMOTE_LOG' '$REMOTE_EVENTS'; df -h /tmp; sed -n '1,8p' /proc/meminfo"
  "$RUN_RUST" launcher 0 >/dev/null
  wait_for_launcher || {
    echo "FAIL: launcher did not start before resource-exhaustion repro" >&2
    exit 1
  }
  launcher_pid >"$OUT/before.pid"
  latest_crash_report >"$OUT/before-crash-report.txt"
}

run_tmp_enospc() {
  echo "==> Filling /tmp until the device reports ENOSPC"
  remote "mkdir -p '$REMOTE_FILL_DIR'; dd if=/dev/zero of='$REMOTE_FILL_FILE' bs=1M count=4096 >'$REMOTE_DD_LOG' 2>&1 || true; df -h /tmp; cat '$REMOTE_DD_LOG' 2>/dev/null || true" | tee "$OUT/pressure.txt"
}

run_memory_pressure() {
  echo "==> Starting bounded memory pressure"
  remote "command -v python3 >/dev/null 2>&1 || command -v python >/dev/null 2>&1" >/dev/null || {
    echo "FAIL: memory-pressure scenario requires python on the MiSTer" >&2
    exit 1
  }
  remote "mkdir -p '$REMOTE_FILL_DIR'; cat >'$REMOTE_MEM_SCRIPT' <<'PY'
import os
import signal
import sys
import time

target_mib = int(os.environ.get('TARGET_MIB', '360'))
chunk_mib = int(os.environ.get('CHUNK_MIB', '8'))
hold_secs = int(os.environ.get('HOLD_SECS', '90'))
chunks = []
running = True

def stop(signum, frame):
    global running
    running = False

signal.signal(signal.SIGTERM, stop)
signal.signal(signal.SIGINT, stop)
print('mister-magik-resource-exhaustion-memory start target_mib=%d chunk_mib=%d hold_secs=%d pid=%d' % (target_mib, chunk_mib, hold_secs, os.getpid()), flush=True)
allocated = 0
try:
    while running and allocated < target_mib:
        chunks.append(bytearray(chunk_mib * 1024 * 1024))
        allocated += chunk_mib
        print('allocated_mib=%d' % allocated, flush=True)
        time.sleep(0.05)
    deadline = time.time() + hold_secs
    while running and time.time() < deadline:
        time.sleep(0.25)
    print('done allocated_mib=%d running=%s' % (allocated, running), flush=True)
except MemoryError:
    print('memory_error allocated_mib=%d' % allocated, flush=True)
    time.sleep(hold_secs)
PY
TARGET_MIB='$MEMORY_TARGET_MIB' CHUNK_MIB='$MEMORY_CHUNK_MIB' HOLD_SECS='$((POLL_SECS + 15))' python3 '$REMOTE_MEM_SCRIPT' >'$REMOTE_MEM_LOG' 2>&1 & echo \$! >'$REMOTE_MEM_PID'; echo memory_pressure_pid=\$(cat '$REMOTE_MEM_PID'); sleep 2; sed -n '1,60p' '$REMOTE_MEM_LOG'; sed -n '1,12p' /proc/meminfo" | tee "$OUT/pressure.txt"
}

poll_launcher() {
  echo "==> Polling launcher for ${POLL_SECS}s (expect=$EXPECT)"
  local before
  before="$(cat "$OUT/before.pid" 2>/dev/null || true)"
  local crashed=0
  local changed=0
  for ((i = 0; i < POLL_SECS; i++)); do
    local current
    current="$(launcher_pid)"
    if [ -z "$current" ]; then
      crashed=1
      echo "launcher_exited_after_secs=$i" | tee "$OUT/result.txt"
      break
    fi
    if [ -n "$before" ] && [ "$current" != "$before" ]; then
      crashed=1
      changed=1
      echo "launcher_restarted_after_secs=$i before_pid=$before current_pid=$current" | tee "$OUT/result.txt"
      break
    fi
    sleep 1
  done

  if [ "$crashed" -eq 0 ]; then
    local before_crash
    local after_crash
    before_crash="$(cat "$OUT/before-crash-report.txt" 2>/dev/null || true)"
    after_crash="$(latest_crash_report)"
    if [ -n "$after_crash" ] && [ "$after_crash" != "$before_crash" ]; then
      crashed=1
      echo "new_crash_report_after_survival_poll=$after_crash before=$before_crash" | tee "$OUT/result.txt"
    elif main_reports_launcher_crashed; then
      crashed=1
      echo "main_reported_launcher_crashed_after_survival_poll=true" | tee "$OUT/result.txt"
    else
      echo "launcher_survived_${POLL_SECS}s=true pid=$(launcher_pid)" | tee "$OUT/result.txt"
    fi
  fi
  echo "crashed=$crashed changed=$changed" >>"$OUT/result.txt"
  return "$crashed"
}

trap 'dump_artifacts; cleanup_remote' EXIT

start_clean_launcher
case "$SCENARIO" in
  tmp-enospc) run_tmp_enospc ;;
  memory-pressure) run_memory_pressure ;;
esac

crashed=0
if ! poll_launcher; then
  crashed=1
fi

dump_artifacts
cleanup_remote
trap - EXIT

"$RUN_RUST" launcher 0 >/dev/null || true
wait_for_launcher || {
  echo "FAIL: launcher did not recover after cleanup" >&2
  exit 1
}

if [ "$EXPECT" = "crash" ] && [ "$crashed" -eq 1 ]; then
  echo "==> Repro matched expectation: launcher crashed under $SCENARIO"
  exit 0
fi

if [ "$EXPECT" = "survive" ] && [ "$crashed" -eq 0 ]; then
  echo "==> Repro matched expectation: launcher survived $SCENARIO"
  exit 0
fi

echo "FAIL: scenario=$SCENARIO expected=$EXPECT crashed=$crashed" >&2
exit 1
