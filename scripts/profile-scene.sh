#!/usr/bin/env bash
# Profile a Slint bench scene on the MiSTer: per-frame phase timings + CPU flamegraph.
#
#   MISTER_IP=192.168.1.117 MISTER_PASS=1 scripts/profile-scene.sh full_motion 30
#   scripts/profile-scene.sh full_motion 30 --skip-build
#
# Artifacts land in history/toolchain-bench/profile-<scene>-<timestamp>/
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
RUST_DIR="$HERE/magik-gui"
BIN="$RUST_DIR/target/armv7-unknown-linux-gnueabihf/release-device-profile/mister-magik-fb"
REMOTE="/media/fat/mister-magik/mister-magik-fb"
SSH="$HERE/scripts/mister_ssh.py"
OUT_ROOT="$HERE/history/toolchain-bench"
ANALYZE="$HERE/scripts/analyze-frame-profile.py"

export MISTER_IP="${MISTER_IP:-192.168.1.117}"
export MISTER_PASS="${MISTER_PASS:-1}"

SCENE="full_motion"
SECS=30
SKIP_BUILD=0
DEPLOY=1
POSITIONAL=()

usage() {
  sed -n '2,6p' "$0" | sed 's/^# \{0,1\}//'
  echo ""
  echo "Usage: profile-scene.sh [scene] [secs] [--skip-build] [--no-deploy]"
  exit "${1:-0}"
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    -h|--help) usage 0 ;;
    --skip-build) SKIP_BUILD=1; shift ;;
    --no-deploy) DEPLOY=0; shift ;;
    -*) echo "Unknown option: $1" >&2; usage 1 ;;
    *) POSITIONAL+=("$1"); shift ;;
  esac
done

SCENE="${POSITIONAL[0]:-$SCENE}"
SECS="${POSITIONAL[1]:-$SECS}"

mister() {
  uv run python "$SSH" "$@"
}

STAMP="$(date -u +%Y%m%dT%H%M%SZ)"
OUT_DIR="$OUT_ROOT/profile-${SCENE}-${STAMP}"
mkdir -p "$OUT_DIR"

if [[ "$SKIP_BUILD" -eq 0 ]]; then
  echo "==> Building release-device-profile + pprof feature..."
  "$RUST_DIR/build-arm.sh" --profile
fi

if [[ ! -f "$BIN" ]]; then
  echo "ERROR: missing $BIN (run build-arm.sh --profile)" >&2
  exit 1
fi

if [[ "$DEPLOY" -eq 1 ]]; then
  echo "==> Deploying profiling binary..."
  mister put "$BIN" "${REMOTE}.new"
  mister run "mv ${REMOTE}.new ${REMOTE} && chmod +x ${REMOTE}"
fi

TAG="${SCENE}-${STAMP}"
FRAME_TSV="/tmp/mister-frame-${TAG}.tsv"
PPROF_SVG="/tmp/mister-pprof-${TAG}.svg"
UI_LOG="/tmp/mister-ui-${TAG}.log"

echo "==> Profiling scene=$SCENE secs=$SECS on device..."
mister run "
set -e
echo -1 > /proc/sys/kernel/perf_event_paranoid 2>/dev/null || true
kill -9 \$(pidof MiSTer) 2>/dev/null || true
MISTER_PROFILE=1 \
MISTER_PROFILE_FILE=$FRAME_TSV \
MISTER_PPROF=1 \
MISTER_PPROF_OUT=$PPROF_SVG \
MISTER_PPROF_HZ=99 \
$REMOTE ui $SCENE $SECS > $UI_LOG 2>&1
RC=\$?
echo ___PROFILE_RC___
echo \$RC
echo ___PROFILE_UI_LOG___
cat $UI_LOG
" | tee "$OUT_DIR/device.log"

if grep -q ___PROFILE_UI_LOG___ "$OUT_DIR/device.log"; then
  sed -n '/___PROFILE_UI_LOG___/,$p' "$OUT_DIR/device.log" | tail -n +2 >"$OUT_DIR/ui.log"
else
  cp "$OUT_DIR/device.log" "$OUT_DIR/ui.log"
fi

echo "==> Pulling artifacts..."
FRAME_LOCAL=""
PPROF_LOCAL=""
for remote in "$FRAME_TSV" "$PPROF_SVG"; do
  base="$(basename "$remote")"
  if mister get "$remote" "$OUT_DIR/$base" 2>/dev/null; then
    echo "    got $base ($(wc -c <"$OUT_DIR/$base" | tr -d ' ') bytes)"
    [[ "$base" == *.tsv ]] && FRAME_LOCAL="$OUT_DIR/$base"
    [[ "$base" == *.svg ]] && PPROF_LOCAL="$OUT_DIR/$base"
  else
    echo "    (missing $base on device)"
  fi
done

{
  echo "scene=$SCENE"
  echo "secs=$SECS"
  echo "stamp=$STAMP"
  echo "binary_bytes=$(stat -f%z "$BIN" 2>/dev/null || stat -c%s "$BIN")"
  echo "render=960x540"
  echo "fb=1920x1080"
} >"$OUT_DIR/run.meta"

if [[ -n "$FRAME_LOCAL" && -f "$ANALYZE" ]]; then
  echo ""
  echo "==> Frame profile analysis:"
  python3 "$ANALYZE" "$FRAME_LOCAL" | tee "$OUT_DIR/analysis.txt"
fi

echo ""
echo "==> Done. Artifacts in $OUT_DIR/"
echo "    ui.log              — raw stdout + frame profile summary"
echo "    analysis.txt        — TSV rollup (if frame TSV present)"
echo "    mister-frame-*.tsv  — per-frame anim/render/vsync/copy/wall"
if [[ -n "$PPROF_LOCAL" && -s "$PPROF_LOCAL" ]]; then
  echo "    mister-pprof-*.svg  — CPU flamegraph (open in browser)"
else
  echo "    (no flamegraph — see ui.log for cpu_profile sample count / errors)"
fi
