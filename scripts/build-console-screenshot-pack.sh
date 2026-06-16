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

Non-canonical scraper/title stems are rejected. Rename them, or produce an
explicit staging directory before running this builder:

  scripts/mister console-screenshot-stage \
    --system saturn \
    --input build/source-screenshots/saturn-scraper \
    --output build/source-screenshots/saturn-canonical \
    --report build/source-screenshots/saturn-stage-report.tsv

Options:
  --deploy         Copy the built pack to /media/fat/mister-magik/assets.
  --work-dir DIR   Local work dir (default build/<system>-screenshots).
  --max-size N     Preview box size (default 320).

Environment:
  MISTER_MAME_SQLITE  Optional validation DB (default build/mame.sqlite3).
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
PACK="$CACHE_DIR/raw565-hybrid-${MAX_SIZE}x${MAX_SIZE}-lz4block-12.mmlz4b"
REMOTE_PACK="/media/fat/mister-magik/assets/${SYSTEM}-screenshots.mmlz4b"
MAME_SQLITE="${MISTER_MAME_SQLITE:-$ROOT/build/mame.sqlite3}"

rm -rf "$CANONICAL" "$CACHE_DIR"
mkdir -p "$CANONICAL" "$CACHE_DIR"

validate_software_name() {
  local source="$1"
  local list="$2"
  local software="$3"

  if [[ "$list" != "$LIST_NAME" ]]; then
    echo "invalid screenshot stem '$source': list '$list' does not match --system $SYSTEM ($LIST_NAME)" >&2
    return 1
  fi
  if [[ ! "$software" =~ ^[a-z0-9_]+$ ]]; then
    echo "invalid screenshot stem '$source': software name must be a MAME short name like 'sonic' or 'sf2ce'" >&2
    return 1
  fi
  if [[ -f "$MAME_SQLITE" ]] && command -v sqlite3 >/dev/null 2>&1; then
    local count
    count="$(sqlite3 "$MAME_SQLITE" "SELECT count(*) FROM mame_software_items WHERE list_name='$list' AND software_name='$software';")"
    if [[ "$count" != "1" ]]; then
      echo "invalid screenshot stem '$source': '$list:$software' is not in $MAME_SQLITE" >&2
      return 1
    fi
  fi
}

found=0
errors=0
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
    if [[ "$stem" =~ ^mame-software__([^_]+)__([a-z0-9_]+)$ ]]; then
      list="${BASH_REMATCH[1]}"
      software="${BASH_REMATCH[2]}"
    else
      echo "invalid screenshot stem '$stem': expected mame-software__${LIST_NAME}__<software_name>" >&2
      errors=$((errors + 1))
      continue
    fi
    canonical="$stem"
  else
    list="$LIST_NAME"
    software="$stem"
    canonical="mame-software__${LIST_NAME}__${stem}"
  fi
  if ! validate_software_name "$stem" "$list" "$software"; then
    errors=$((errors + 1))
    continue
  fi
  cp "$file" "$CANONICAL/${canonical}.${ext_lower}"
  found=$((found + 1))
done < <(find "$INPUT_DIR" -maxdepth 1 -type f -print0)

if [[ "$errors" != "0" ]]; then
  echo "refusing to build $SYSTEM screenshot pack: $errors invalid source screenshot(s)" >&2
  exit 1
fi
if [[ "$found" == "0" ]]; then
  echo "no source screenshots found in $INPUT_DIR" >&2
  exit 1
fi

echo "==> Building raw565 preview cache for $SYSTEM"
"$MISTER" preview-cache-build --input "$CANONICAL" --output "$CACHE_DIR" --max "$MAX_SIZE"

if [[ "$DEPLOY" == "1" ]]; then
  echo "==> Deploying $PACK to $REMOTE_PACK"
  "$MISTER" run "mkdir -p '/media/fat/mister-magik/assets'"
  "$MISTER" put "$PACK" "$REMOTE_PACK"
fi

echo "console_screenshot_pack system=$SYSTEM list=$LIST_NAME entries=$found pack=$PACK deploy=$DEPLOY"
