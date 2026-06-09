#!/usr/bin/env bash
# Build the profiling binary, run a scene with pprof, and pull the SVG flamegraph.
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
MISTER="$HERE/scripts/mister"
RUST_DIR="$HERE/magik-gui"
OUT_DIR="$HERE/build/cpu-flamegraphs"

usage() {
  cat <<'EOF'
Usage: scripts/cpu-flamegraph-scene.sh SCENE [SECS] [LABEL]

Builds `magik-gui/build-arm.sh --profile`, deploys that profiling binary, runs
the scene with `MISTER_PPROF=1`, and pulls the SVG flamegraph.

Note: MiSTer perf_event permissions can produce 0 samples. If that happens,
use frame TSV/trace reports first, or check /proc/sys/kernel/perf_event_paranoid.
EOF
}

scene="${1:-}"
secs="${2:-10}"
label="${3:-${scene:-scene}-$(date -u +%Y%m%dT%H%M%SZ)}"
if [[ -z "$scene" || "$scene" == "-h" || "$scene" == "--help" ]]; then
  usage
  exit 0
fi

mkdir -p "$OUT_DIR"
remote_svg="/tmp/${label}-cpu.svg"
remote_log="/tmp/${label}-cpu-profile.log"
local_svg="$OUT_DIR/${label}-cpu.svg"
local_log="$OUT_DIR/${label}-cpu-profile.log"
bin="$RUST_DIR/target/armv7-unknown-linux-gnueabihf/release-device-profile/mister-magik-fb"

echo "==> Build profiling binary"
"$RUST_DIR/build-arm.sh" --profile

echo "==> Deploy profiling binary"
"$MISTER" run "kill -9 \$(pidof mister-magik-fb) 2>/dev/null || true; mkdir -p /media/fat/mister-magik"
"$MISTER" put "$bin" /media/fat/mister-magik/mister-magik-fb
"$MISTER" run "chmod +x /media/fat/mister-magik/mister-magik-fb"

echo "==> Run CPU profiler scene=$scene secs=$secs"
"$MISTER" run "kill -9 \$(pidof mister-magik-fb) 2>/dev/null || true; kill -9 \$(pidof MiSTer_MagiK) 2>/dev/null || true; kill -9 \$(pidof MiSTer) 2>/dev/null || true; sleep 5; MISTER_PPROF=1 MISTER_PPROF_OUT=$remote_svg /media/fat/mister-magik/mister-magik-fb ui $scene $secs >$remote_log 2>&1; grep 'cpu_profile:' $remote_log || true"

echo "==> Pull CPU profile artifacts"
"$MISTER" get "$remote_log" "$local_log" || true
if "$MISTER" get "$remote_svg" "$local_svg" >/dev/null 2>&1; then
  echo "wrote $local_svg"
else
  echo "no flamegraph SVG produced; see $local_log" >&2
fi
