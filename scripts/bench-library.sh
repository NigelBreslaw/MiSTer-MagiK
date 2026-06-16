#!/usr/bin/env bash
# Benchmark MiSTer library scan/index performance on the device.
#
#   scripts/bench-library.sh LIB-BASE-20260605 --device --replace-label
#
# Appends library_scan_bench_tsv rows to history/toolchain-bench/results-library.tsv.
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
RUST_DIR="$HERE/magik-gui"
BUILD_PROFILE=release-device
BUILD_FLAG=(--device)
REMOTE="/media/fat/mister-magik/mister-magik-fb"
REMOTE_DIR="/media/fat/mister-magik"
DEPLOY_LOCK="$REMOTE_DIR/deploy.lock"
BENCH_SQLITE="/media/fat/mister-magik/library-scan-bench.sqlite3"
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
  remote_run "if [ -p /dev/MiSTer_cmd ] && pidof MiSTer_MagiK >/dev/null 2>&1; then printf '$1\n' > /dev/MiSTer_cmd; fi" >/dev/null 2>&1 || true
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
trap cleanup_deploy_lock EXIT
remote_run "mkdir -p '$REMOTE_DIR'; : > '$DEPLOY_LOCK'"
magik_command "mister_magik_suspend"
"$HERE/scripts/mister" put "$BIN" "$REMOTE.upload"
remote_run "mv '$REMOTE.upload' '$REMOTE'; chmod +x '$REMOTE'; rm -f '$DEPLOY_LOCK'"
magik_command "mister_magik_resume"
trap - EXIT

echo "== library-scan-bench on device =="
remote_env="MISTER_LIBRARY_BENCH_LABEL=$LABEL MISTER_LIBRARY_BENCH_ITERATIONS=$ITERATIONS MISTER_LIBRARY_BENCH_SQLITE=$BENCH_SQLITE MISTER_LIBRARY_SQLITE=$BENCH_SQLITE"
if [[ "$PRECOUNT" -eq 1 ]]; then
  remote_env="$remote_env MISTER_LIBRARY_BENCH_PRECOUNT=1"
fi
if [[ -n "$SQLITE_BUILD_DIR" ]]; then
  remote_env="$remote_env MISTER_LIBRARY_SQLITE_BUILD_DIR=$SQLITE_BUILD_DIR"
fi
OUT=$(run_with_launcher_suspended "chmod +x $REMOTE; $remote_env $REMOTE library-scan-bench" 2>&1) || true
echo "$OUT"

echo "$OUT" | awk -F '\t' '$1 == "library_scan_bench_tsv" { print $2 "\t" $3 "\t" $4 "\t" $5 "\t" $6 }' >> "$TSV"
if [[ "$POST_REBOOT" -eq 1 ]]; then
  echo "== post-reboot no-change refresh =="
  "$HERE/scripts/mister" reboot-wait
  OUT=$(run_with_launcher_suspended "MISTER_LIBRARY_SQLITE=$BENCH_SQLITE $REMOTE library-refresh" 2>&1) || true
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
      if (us != "") print label "\tpost-reboot\tpost_reboot_rescan\t" us "\t" notes
    }
  ' >> "$TSV"
fi
echo "appended to $TSV"
