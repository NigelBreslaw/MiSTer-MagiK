#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PATCH="$ROOT/experiments/fpga-vblank-latch/Menu_MiSTer-vblank-latched-fbuf.patch"
OUT_DIR="$ROOT/build/fpga-vblank-latch"
WORK_DIR="${MISTER_MENU_BUILD_DIR:-$OUT_DIR/Menu_MiSTer-vblank-latch-work}"
MENU_DIR="${MISTER_MENU_DIR:-${ROOT}/../Menu_MiSTer}"
RBF_OUT="$OUT_DIR/menu-magik-vblank-latch.rbf"
META_OUT="$OUT_DIR/menu-magik-vblank-latch.metadata.txt"
LOG_OUT="$OUT_DIR/menu-magik-vblank-latch.build.log"

usage() {
  cat <<'EOF'
Usage:
  scripts/build-fpga-vblank-latch-core.sh

Builds an experimental Menu_MiSTer RBF with the MiSTer MagiK vblank-latched
framebuffer patch. Set MISTER_MENU_DIR to a writable Menu_MiSTer checkout.
The gitignored reference/Menu_MiSTer checkout is intentionally rejected.
EOF
}

case "${1:-}" in
  -h|--help)
    usage
    exit 0
    ;;
  "")
    ;;
  *)
    echo "unknown argument: $1" >&2
    usage >&2
    exit 2
    ;;
esac

abs_path() {
  (cd "$1" && pwd -P)
}

if [[ ! -d "$MENU_DIR" ]]; then
  echo "missing Menu_MiSTer checkout: $MENU_DIR" >&2
  echo "set MISTER_MENU_DIR to a writable Menu_MiSTer checkout" >&2
  exit 1
fi

MENU_ABS="$(abs_path "$MENU_DIR")"
REFERENCE_ABS="$(abs_path "$ROOT/reference/Menu_MiSTer")"
if [[ "$MENU_ABS" == "$REFERENCE_ABS" ]]; then
  echo "refusing to build from read-only reference checkout: $MENU_ABS" >&2
  exit 1
fi

if [[ ! -f "$MENU_ABS/menu.qsf" || ! -f "$MENU_ABS/sys/sys_top.v" ]]; then
  echo "not a Menu_MiSTer checkout: $MENU_ABS" >&2
  exit 1
fi

if ! command -v quartus_sh >/dev/null 2>&1; then
  echo "missing quartus_sh; install Quartus 17.x or put quartus_sh on PATH" >&2
  exit 1
fi

mkdir -p "$OUT_DIR"
rm -rf "$WORK_DIR"
mkdir -p "$WORK_DIR"
rsync -a --delete \
  --exclude .git \
  --exclude output_files \
  --exclude db \
  --exclude incremental_db \
  "$MENU_ABS"/ "$WORK_DIR"/

if git -C "$WORK_DIR" apply --check "$PATCH" >/dev/null 2>&1; then
  git -C "$WORK_DIR" apply "$PATCH"
elif git -C "$WORK_DIR" apply --reverse --check "$PATCH" >/dev/null 2>&1; then
  echo "patch already applied in work tree"
else
  echo "patch does not apply cleanly to $MENU_ABS" >&2
  git -C "$WORK_DIR" apply --check "$PATCH"
fi

{
  echo "source_dir=$MENU_ABS"
  git -C "$MENU_ABS" rev-parse HEAD 2>/dev/null | sed 's/^/source_commit=/'
  git -C "$MENU_ABS" status --short 2>/dev/null | sed 's/^/source_status=/'
  shasum -a 256 "$PATCH" | awk '{print "patch_sha256="$1}'
  echo "work_dir=$WORK_DIR"
  echo "quartus_sh=$(command -v quartus_sh)"
} > "$META_OUT"

(
  cd "$WORK_DIR"
  quartus_sh --flow compile menu
) 2>&1 | tee "$LOG_OUT"

RBF_CANDIDATE="$WORK_DIR/output_files/menu.rbf"
if [[ ! -f "$RBF_CANDIDATE" ]]; then
  RBF_CANDIDATE="$(find "$WORK_DIR" -path '*/output_files/*.rbf' -print -quit)"
fi
if [[ -z "${RBF_CANDIDATE:-}" || ! -f "$RBF_CANDIDATE" ]]; then
  echo "Quartus completed but no RBF was found under $WORK_DIR/output_files" >&2
  exit 1
fi

cp "$RBF_CANDIDATE" "$RBF_OUT"
shasum -a 256 "$RBF_OUT" | awk '{print "rbf_sha256="$1}' >> "$META_OUT"
echo "rbf=$RBF_OUT" >> "$META_OUT"
echo "$RBF_OUT"
