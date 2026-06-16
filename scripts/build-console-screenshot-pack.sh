#!/usr/bin/env bash
# Build a canonical console screenshot pack from MagiK-owned source images.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
MISTER="$ROOT/scripts/mister"

SYSTEM=""
INPUT_DIR=""
WORK_DIR=""
MAX_SIZE="320"
DEPLOY=0

usage() {
  cat <<'USAGE'
usage: scripts/build-console-screenshot-pack.sh --system SYSTEM --input DIR [options]

SYSTEM is one of: nes, snes, n64, sms, megadrive, saturn.

Source image stems must be canonical MagiK identities:
  mame-software__megadriv__sonic.png

or the software short name for the selected system:
  sonic.png

Options:
  --deploy         Copy the built pack to /media/fat/mister-magik/assets.
  --work-dir DIR   Local work dir (default build/<system>-screenshots).
  --max-size N     Preview box size (default 320).
USAGE
}

while (($#)); do
  case "$1" in
    --system)
      SYSTEM="${2:?--system needs a value}"
      shift 2
      ;;
    --input)
      INPUT_DIR="${2:?--input needs a dir}"
      shift 2
      ;;
    --work-dir)
      WORK_DIR="${2:?--work-dir needs a dir}"
      shift 2
      ;;
    --max-size)
      MAX_SIZE="${2:?--max-size needs a number}"
      shift 2
      ;;
    --deploy)
      DEPLOY=1
      shift
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      echo "unknown option: $1" >&2
      usage >&2
      exit 2
      ;;
  esac
done

case "$SYSTEM" in
  nes) LIST_NAME="nes" ;;
  snes) LIST_NAME="snes" ;;
  n64) LIST_NAME="n64" ;;
  sms) LIST_NAME="sms" ;;
  megadrive) LIST_NAME="megadriv" ;;
  saturn) LIST_NAME="saturn" ;;
  *)
    echo "--system must be one of: nes, snes, n64, sms, megadrive, saturn" >&2
    exit 2
    ;;
esac

if [[ -z "$INPUT_DIR" || ! -d "$INPUT_DIR" ]]; then
  echo "--input must point at a directory of screenshots" >&2
  exit 2
fi
case "$MAX_SIZE" in
  ''|*[!0-9]*)
    echo "--max-size must be a positive integer" >&2
    exit 2
    ;;
esac
if [[ "$MAX_SIZE" == "0" ]]; then
  echo "--max-size must be greater than zero" >&2
  exit 2
fi

WORK_DIR="${WORK_DIR:-$ROOT/build/${SYSTEM}-screenshots}"
CANONICAL="$WORK_DIR/canonical"
CACHE_DIR="$WORK_DIR/cache"
RAW_DIR="$CACHE_DIR/raw565-hybrid-${MAX_SIZE}x${MAX_SIZE}"
PACK="$WORK_DIR/${SYSTEM}-screenshots.mmlz4b"
REMOTE_PACK="/media/fat/mister-magik/assets/${SYSTEM}-screenshots.mmlz4b"

rm -rf "$CANONICAL" "$CACHE_DIR"
mkdir -p "$CANONICAL" "$CACHE_DIR"

found=0
while IFS= read -r -d '' file; do
  name="$(basename "$file")"
  ext="${name##*.}"
  ext_lower="$(printf '%s' "$ext" | tr '[:upper:]' '[:lower:]')"
  stem="${name%.*}"
  case "$ext_lower" in
    png|jpg|jpeg) ;;
    *) continue ;;
  esac
  if [[ "$stem" == mame-software__* ]]; then
    canonical="$stem"
  else
    canonical="mame-software__${LIST_NAME}__${stem}"
  fi
  cp "$file" "$CANONICAL/${canonical}.${ext_lower}"
  found=$((found + 1))
done < <(find "$INPUT_DIR" -maxdepth 1 -type f -print0)

if [[ "$found" == "0" ]]; then
  echo "no source screenshots found in $INPUT_DIR" >&2
  exit 1
fi

echo "==> Building raw565 preview cache for $SYSTEM"
"$MISTER" preview-cache-build --input "$CANONICAL" --output "$CACHE_DIR" --max "$MAX_SIZE"

echo "==> Packing $RAW_DIR"
node "$ROOT/scripts/build-preview-archive.mjs" "$RAW_DIR" "$PACK" lz4-block 12

if [[ "$DEPLOY" == "1" ]]; then
  echo "==> Deploying $PACK to $REMOTE_PACK"
  "$MISTER" run "mkdir -p '/media/fat/mister-magik/assets'"
  "$MISTER" put "$PACK" "$REMOTE_PACK"
fi

echo "console_screenshot_pack system=$SYSTEM list=$LIST_NAME entries=$found pack=$PACK deploy=$DEPLOY"
