#!/usr/bin/env bash
# Fetch Neo Geo screenshots from a MiSTer, build raw565 previews, and pack them.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
MISTER="$ROOT/scripts/mister"

REMOTE_SOURCE="/media/fat/games/NEOGEO/screenshots"
WORK_DIR="$ROOT/build/neogeo-screenshots"
REMOTE_PACK="/media/fat/mister-magik/assets/neogeo-screenshots.mmlz4b"
MAX_SIZE="320"
FETCH=1
DEPLOY=0

usage() {
  cat <<'USAGE'
usage: scripts/build-neogeo-screenshot-pack.sh [options]

Options:
  --skip-fetch             Use existing build/neogeo-screenshots/originals files.
  --deploy                 Copy the built pack to /media/fat/mister-magik/assets.
  --remote-source PATH     MiSTer source dir (default /media/fat/games/NEOGEO/screenshots).
  --remote-pack PATH       MiSTer deploy path (default /media/fat/mister-magik/assets/neogeo-screenshots.mmlz4b).
  --work-dir PATH          Local work dir (default build/neogeo-screenshots).
  --max-size N             Preview box size (default 320).
USAGE
}

while (($#)); do
  case "$1" in
    --skip-fetch)
      FETCH=0
      shift
      ;;
    --deploy)
      DEPLOY=1
      shift
      ;;
    --remote-source)
      REMOTE_SOURCE="${2:?--remote-source needs a path}"
      shift 2
      ;;
    --remote-pack)
      REMOTE_PACK="${2:?--remote-pack needs a path}"
      shift 2
      ;;
    --work-dir)
      WORK_DIR="${2:?--work-dir needs a path}"
      shift 2
      ;;
    --max-size)
      MAX_SIZE="${2:?--max-size needs a number}"
      shift 2
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

ORIGINALS="$WORK_DIR/originals"
CACHE_DIR="$WORK_DIR/cache"
RAW_DIR="$CACHE_DIR/raw565-hybrid-${MAX_SIZE}x${MAX_SIZE}"
PACK="$WORK_DIR/neogeo-screenshots.mmlz4b"

mkdir -p "$ORIGINALS" "$CACHE_DIR"

if [[ "$FETCH" == "1" ]]; then
  echo "==> Fetching Neo Geo screenshots from $REMOTE_SOURCE"
  mapfile -t REMOTE_FILES < <("$MISTER" run "find '$REMOTE_SOURCE' -maxdepth 1 -type f \\( -iname '*.png' -o -iname '*.jpg' -o -iname '*.jpeg' \\) | sort")
  if [[ "${#REMOTE_FILES[@]}" == "0" ]]; then
    echo "no screenshots found under $REMOTE_SOURCE" >&2
    exit 1
  fi
  for remote in "${REMOTE_FILES[@]}"; do
    name="$(basename "$remote")"
    "$MISTER" get "$remote" "$ORIGINALS/$name"
  done
fi

if ! find "$ORIGINALS" -type f \( -iname '*.png' -o -iname '*.jpg' -o -iname '*.jpeg' \) -print -quit | grep -q .; then
  echo "no local source screenshots in $ORIGINALS" >&2
  exit 1
fi

echo "==> Building raw565 preview cache"
"$MISTER" preview-cache-build --input "$ORIGINALS" --output "$CACHE_DIR" --max "$MAX_SIZE"

echo "==> Packing $RAW_DIR"
node "$ROOT/scripts/build-preview-archive.mjs" "$RAW_DIR" "$PACK" lz4-block 12

if [[ "$DEPLOY" == "1" ]]; then
  remote_dir="$(dirname "$REMOTE_PACK")"
  echo "==> Deploying $PACK to $REMOTE_PACK"
  "$MISTER" run "mkdir -p '$remote_dir'"
  "$MISTER" put "$PACK" "$REMOTE_PACK"
fi

echo "neogeo_screenshot_pack originals=$ORIGINALS raw565=$RAW_DIR pack=$PACK deploy=$DEPLOY"
