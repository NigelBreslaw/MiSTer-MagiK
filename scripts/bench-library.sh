#!/usr/bin/env bash
# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

# Benchmark MiSTer library-refresh scan/index performance on the device.
#
#   scripts/bench-library.sh LIB-BASE-20260605 --device --replace-label
#
# Appends production library-refresh timing rows to history/toolchain-bench/results-library.tsv.
set -euo pipefail

echo "ERROR: bench-library targeted the retired V2 monolith; use bench-catalog-rebuild.sh" >&2
exit 2

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
RUST_DIR="$HERE/magik-gui"
BUILD_PROFILE=release-device
BUILD_FLAG=(--device)
REMOTE="/media/fat/mister-magik-dev/mister-magik-fb"
REMOTE_DIR="/media/fat/mister-magik-dev"
DEPLOY_LOCK="$REMOTE_DIR/deploy.lock"
BENCH_SQLITE="/media/fat/mister-magik-dev/library-scan-bench.sqlite3"
BENCH_DIR="$HERE/history/toolchain-bench"
TSV="$BENCH_DIR/results-library.tsv"
ITERATIONS=3
LABEL="LIB-BENCH"
DO_CLEAN=0
SKIP_BUILD=0
REPLACE_LABEL=0
POST_REBOOT=0
PRECOUNT=0
SQLITE_BUILD_DIR=""

usage() {
  sed -n '2,8p' "$0" | sed 's/^# \{0,1\}//'
  echo ""
  echo "Options: --clean  --skip-build  --replace-label  --device  --iterations N  --post-reboot  --precount  --sqlite-build-dir DIR  -h"
  exit "${1:-0}"
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    -h|--help) usage 0 ;;
    --clean) DO_CLEAN=1; shift ;;
    --skip-build) SKIP_BUILD=1; shift ;;
    --replace-label) REPLACE_LABEL=1; shift ;;
    --device) BUILD_PROFILE=release-device; shift ;;
    --iterations) ITERATIONS="${2:?}"; shift 2 ;;
    --post-reboot) POST_REBOOT=1; shift ;;
    --precount) PRECOUNT=1; shift ;;
    --sqlite-build-dir) SQLITE_BUILD_DIR="${2:?}"; shift 2 ;;
    -*) echo "Unknown option: $1" >&2; usage 1 ;;
    *) LABEL="$1"; shift ;;
  esac
done

BIN="$RUST_DIR/target/armv7-unknown-linux-gnueabihf/$BUILD_PROFILE/mister-magik-fb"
mkdir -p "$BENCH_DIR"

remote_run() {
  "$HERE/scripts/mister" run "$1"
}

magik_command() {
  remote_run "if [ -p /dev/MiSTer_cmd ] && pidof MiSTer_MagiKDev >/dev/null 2>&1; then printf '$1\n' > /dev/MiSTer_cmd; fi" >/dev/null 2>&1 || true
}

cleanup_deploy_lock() {
  remote_run "rm -f '$DEPLOY_LOCK'" >/dev/null 2>&1 || true
  magik_command "mister_magik_resume"
}

run_with_launcher_suspended() {
  trap 'magik_command "mister_magik_resume"' RETURN
  magik_command "mister_magik_suspend"
  remote_run "$1"
  magik_command "mister_magik_resume"
  trap - RETURN
}

if [[ ! -f "$TSV" ]]; then
  echo "label	iteration	scenario	us	notes" > "$TSV"
elif [[ "$REPLACE_LABEL" -eq 1 ]]; then
  tmp="$(mktemp)"
  { head -1 "$TSV"; grep -v "^${LABEL}	" "$TSV" | tail -n +2; } > "$tmp" || true
  mv "$tmp" "$TSV"
fi

if [[ "$SKIP_BUILD" -eq 0 ]]; then
  echo "== build ($BUILD_PROFILE) =="
  if [[ "$DO_CLEAN" -eq 1 ]]; then
    (cd "$RUST_DIR" && ./build-arm.sh "${BUILD_FLAG[@]}" --clean)
  else
    (cd "$RUST_DIR" && ./build-arm.sh "${BUILD_FLAG[@]}")
  fi
fi

echo "== deploy =="
if [[ ! -f "$BIN" ]]; then
  echo "missing built binary: $BIN" >&2
  exit 1
fi
"$HERE/scripts/mister" deploy-magik-bin "$BIN" "$REMOTE" >/dev/null

echo "== production library-refresh on device =="
if [[ -n "$SQLITE_BUILD_DIR" ]]; then
  sqlite_build_env="MISTER_LIBRARY_SQLITE_BUILD_DIR=$SQLITE_BUILD_DIR"
else
  sqlite_build_env=""
fi
for iteration in $(seq 1 "$ITERATIONS"); do
  echo "== iteration $iteration/$ITERATIONS =="
  remote_run "rm -f '$BENCH_SQLITE'"
  remote_env="MISTER_LIBRARY_BENCH_LABEL=$LABEL MISTER_LIBRARY_BENCH_ACTIVE_ITERATION=$iteration MISTER_LIBRARY_SQLITE=$BENCH_SQLITE MISTER_MAGIK_FOREGROUND_LIBRARY_REFRESH=1"
  if [[ -n "$sqlite_build_env" ]]; then
    remote_env="$remote_env $sqlite_build_env"
  fi
  OUT=$(run_with_launcher_suspended "chmod +x $REMOTE; $remote_env $REMOTE library-refresh" 2>&1) || true
  echo "$OUT"
  echo "$OUT" | awk -v label="$LABEL" -v iteration="$iteration" -F '\t' '
    BEGIN { OFS = "\t" }
    $1 == "library_scan_timing" {
      print label, iteration, "scan_stage_" $2, int(($3 + 500) / 1000), $4
    }
    $1 == "library_import_timing" {
      print label, iteration, "import_stage_" $2, int(($3 + 500) / 1000), $4
    }
    $1 == "library_sqlite_publish_tsv" {
      print label, iteration, "sqlite_publish_" $4, $11, "bytes=" $5 " copy_ms=" $7 " build_sync_ms=" $6 " final_sync_ms=" $8 " rename_ms=" $9 " parent_sync_ms=" $10 " progress_events=" $12 " result=" $13
    }
    $1 == "library_refresh" && $2 == "done" {
      n = split($3, parts, " ")
      scan_us = ""
      import_us = ""
      bytes = ""
      for (i = 1; i <= n; i++) {
        if (parts[i] ~ /^scan_us=/) { scan_us = parts[i]; sub(/^scan_us=/, "", scan_us) }
        if (parts[i] ~ /^import_us=/) { import_us = parts[i]; sub(/^import_us=/, "", import_us) }
        if (parts[i] ~ /^bytes=/) { bytes = parts[i]; sub(/^bytes=/, "", bytes) }
      }
      if (scan_us != "") print label, iteration, "refresh_scan", scan_us, $3
      if (import_us != "") print label, iteration, "refresh_import", import_us, $3
      print label, iteration, "refresh_done", 0, "bytes=" bytes " " $3
    }
    $1 == "library_refresh" && $2 == "failed" {
      print label, iteration, "refresh_failed", 0, $3
      failed = 1
    }
    END { exit failed ? 1 : 0 }
  ' >> "$TSV"
done
if [[ "$POST_REBOOT" -eq 1 ]]; then
  echo "== post-reboot explicit full rebuild =="
  "$HERE/scripts/mister" reboot-wait --direct-reset
  OUT=$(run_with_launcher_suspended "MISTER_LIBRARY_SQLITE=$BENCH_SQLITE MISTER_MAGIK_FOREGROUND_LIBRARY_REFRESH=1 $REMOTE library-refresh" 2>&1) || true
  echo "$OUT"
  echo "$OUT" | awk -v label="$LABEL" -F '\t' '
    $1 == "library_refresh" && $2 == "done" {
      notes = $3
      us = ""
      n = split($3, parts, " ")
      for (i = 1; i <= n; i++) {
        if (parts[i] ~ /^scan_us=/) {
          us = parts[i]
          sub(/^scan_us=/, "", us)
        }
      }
      if (us != "") print label "\tpost-reboot\tpost_reboot_force_build\t" us "\t" notes
    }
  ' >> "$TSV"
fi
echo "appended to $TSV"
