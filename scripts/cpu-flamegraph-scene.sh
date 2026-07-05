#!/usr/bin/env bash
# Build the profiling binary, verify pprof sampling, run a scene, and pull the SVG flamegraph.
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
MISTER="$HERE/scripts/mister"
source "$HERE/scripts/mister-supervision-lib.sh"
RUST_DIR="$HERE/magik-gui"
OUT_DIR="$HERE/build/cpu-flamegraphs"

usage() {
  cat <<'EOF'
Usage: scripts/cpu-flamegraph-scene.sh SCENE [SECS] [LABEL]

Builds a `magik-gui/build-arm.sh --profile` binary, deploys it, runs
an in-process CPU sampling smoke test, then runs the scene with
`MISTER_PPROF=1` and pulls the SVG flamegraph.

The profiler uses SIGPROF/ITIMER_PROF sampling, not the device-side `perf` CLI.
If the smoke test reports 0 samples, the script exits before running the scene.
EOF
}

scene="${1:-}"
secs="${2:-10}"
label="${3:-${scene:-scene}-$(date -u +%Y%m%dT%H%M%SZ)}"
if [[ -z "$scene" || "$scene" == "-h" || "$scene" == "--help" ]]; then
  usage
  exit 0
fi
if [[ ! "$scene" =~ ^[A-Za-z0-9_.-]+$ ]]; then
  echo "scene must contain only letters, numbers, _, ., or -" >&2
  exit 2
fi
if [[ ! "$secs" =~ ^[0-9]+$ ]]; then
  echo "secs must be an integer number of seconds" >&2
  exit 2
fi
if [[ ! "$label" =~ ^[A-Za-z0-9_.-]+$ ]]; then
  echo "label must contain only letters, numbers, _, ., or -" >&2
  exit 2
fi

mkdir -p "$OUT_DIR"
remote_svg="/tmp/${label}-cpu.svg"
remote_log="/tmp/${label}-cpu-profile.log"
remote_smoke_svg="/tmp/${label}-cpu-smoke.svg"
remote_smoke_log="/tmp/${label}-cpu-smoke.log"
local_svg="$OUT_DIR/${label}-cpu.svg"
local_log="$OUT_DIR/${label}-cpu-profile.log"
local_smoke_svg="$OUT_DIR/${label}-cpu-smoke.svg"
local_smoke_log="$OUT_DIR/${label}-cpu-smoke.log"
bin="$RUST_DIR/target/armv7-unknown-linux-gnueabihf/release-device-profile/mister-magik-fb"
build_args=(--profile)
case "$scene" in
  arcade)
    echo "The direct arcade scene was removed. Profile the real Arcade screen through scripts/profile-preview-scroll.sh and launcher traces." >&2
    exit 2
    ;;
  launcher|controller_test)
    build_args+=(--ui-scope launcher)
    ;;
  *)
    build_args+=(--all-scenes)
    ;;
esac

echo "==> Build profiling binary"
"$RUST_DIR/build-arm.sh" "${build_args[@]}"

echo "==> Deploy profiling binary"
mister_suspend_launcher
trap 'mister_restart_launcher >/dev/null 2>&1 || true' EXIT
"$MISTER" run "kill -9 \$(pidof mister-magik-fb) 2>/dev/null || true; mkdir -p /media/fat/mister-magik"
"$MISTER" put "$bin" /media/fat/mister-magik/mister-magik-fb
"$MISTER" run "chmod +x /media/fat/mister-magik/mister-magik-fb"

echo "==> Run CPU profiler smoke"
if ! "$MISTER" run "kill -9 \$(pidof mister-magik-fb) 2>/dev/null || true; rm -f $remote_smoke_svg $remote_smoke_log; MISTER_PPROF=1 MISTER_PPROF_OUT=$remote_smoke_svg /media/fat/mister-magik/mister-magik-fb cpu-profile-smoke 3 >$remote_smoke_log 2>&1; status=\$?; grep 'cpu_profile' $remote_smoke_log || true; test -s $remote_smoke_svg || status=1; exit \$status"; then
  "$MISTER" get "$remote_smoke_log" "$local_smoke_log" || true
  echo "cpu profiler smoke failed; see $local_smoke_log" >&2
  exit 1
fi
"$MISTER" get "$remote_smoke_log" "$local_smoke_log" || true
if "$MISTER" get "$remote_smoke_svg" "$local_smoke_svg" >/dev/null 2>&1; then
  echo "smoke wrote $local_smoke_svg"
else
  echo "cpu profiler smoke passed but SVG pull failed; see $local_smoke_log" >&2
  exit 1
fi

echo "==> Run CPU profiler scene=$scene secs=$secs"
if ! "$MISTER" run "kill -9 \$(pidof mister-magik-fb) 2>/dev/null || true; rm -f $remote_svg $remote_log; sleep 5; MISTER_PPROF=1 MISTER_PPROF_OUT=$remote_svg /media/fat/mister-magik/mister-magik-fb ui $scene $secs >$remote_log 2>&1; status=\$?; grep 'cpu_profile:' $remote_log || true; test -s $remote_svg || status=1; exit \$status"; then
  "$MISTER" get "$remote_log" "$local_log" || true
  echo "cpu profiler scene failed; see $local_log" >&2
  exit 1
fi

echo "==> Pull CPU profile artifacts"
"$MISTER" get "$remote_log" "$local_log" || true
if "$MISTER" get "$remote_svg" "$local_svg" >/dev/null 2>&1; then
  if [ -s "$local_svg" ]; then
    echo "wrote $local_svg"
  else
    echo "pulled empty flamegraph SVG; see $local_log" >&2
    exit 1
  fi
else
  echo "no flamegraph SVG produced; see $local_log" >&2
  exit 1
fi
