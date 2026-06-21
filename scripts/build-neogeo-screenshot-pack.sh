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
COLLAPSE_FAMILIES=1
STAGE_ONLY=0
MAME_SQLITE="${MISTER_MAME_SQLITE:-$ROOT/build/mame.sqlite3}"

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
  --mame-sqlite PATH       MAME metadata DB for parent-family collapse (default build/mame.sqlite3).
  --no-family-collapse     Keep source stems instead of collapsing clones to parent families.
  --stage-only             Rebuild the family-collapsed staging dir, then exit.
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
    --mame-sqlite)
      MAME_SQLITE="${2:?--mame-sqlite needs a path}"
      shift 2
      ;;
    --no-family-collapse)
      COLLAPSE_FAMILIES=0
      shift
      ;;
    --stage-only)
      STAGE_ONLY=1
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
CANDIDATES="$WORK_DIR/family"
CACHE_DIR="$WORK_DIR/cache"
RAW_DIR="$CACHE_DIR/raw565-hybrid-${MAX_SIZE}x${MAX_SIZE}"
PACK="$CACHE_DIR/${MAX_SIZE}x${MAX_SIZE}-screenshots.mmlz4b"

mkdir -p "$ORIGINALS"

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

FAMILY_LOOKUP=0
if [[ "$COLLAPSE_FAMILIES" == "1" ]]; then
  if [[ ! -f "$MAME_SQLITE" ]]; then
    echo "warning: MAME sqlite not found at $MAME_SQLITE; keeping Neo Geo screenshot stems" >&2
  elif ! command -v sqlite3 >/dev/null 2>&1; then
    echo "warning: sqlite3 not found; keeping Neo Geo screenshot stems" >&2
  elif ! sqlite3 "$MAME_SQLITE" "SELECT count(*) FROM sqlite_master WHERE type='table' AND name='mame_machines';" >/dev/null 2>&1; then
    echo "warning: $MAME_SQLITE has no mame_machines table; keeping Neo Geo screenshot stems" >&2
  else
    FAMILY_LOOKUP=1
  fi
fi

sql_quote() {
  printf "%s" "$1" | sed "s/'/''/g"
}

family_stem_for() {
  local stem="$1"
  local alias escaped family

  if [[ "$FAMILY_LOOKUP" != "1" ]]; then
    printf "%s" "$stem"
    return
  fi

  IFS=',' read -ra aliases <<<"$stem"
  for alias in "${aliases[@]}"; do
    if [[ ! "$alias" =~ ^[A-Za-z0-9_]+$ ]]; then
      continue
    fi
    escaped="$(sql_quote "$alias")"
    family="$(
      sqlite3 -noheader -batch "$MAME_SQLITE" \
        "SELECT COALESCE(NULLIF(parent_setname,''), setname) FROM mame_machines WHERE setname='$escaped' LIMIT 1;"
    )"
    if [[ -n "$family" ]]; then
      printf "%s" "$family"
      return
    fi
  done

  printf "%s" "$stem"
}

existing_candidate_path() {
  local stem="$1"
  local ext path
  for ext in png jpg jpeg; do
    path="$CANDIDATES/$stem.$ext"
    if [[ -f "$path" ]]; then
      printf "%s" "$path"
      return 0
    fi
  done
  return 1
}

remove_candidate_path() {
  local stem="$1"
  rm -f "$CANDIDATES/$stem.png" "$CANDIDATES/$stem.jpg" "$CANDIDATES/$stem.jpeg"
}

echo "==> Staging Neo Geo screenshots by MAME family"
rm -rf "$CANDIDATES"
if [[ "$STAGE_ONLY" != "1" ]]; then
  rm -rf "$CACHE_DIR"
fi
mkdir -p "$CANDIDATES" "$CACHE_DIR"

source_images=0
collapsed_images=0
duplicate_family_images=0
parent_replacements=0
while IFS= read -r file; do
  name="$(basename "$file")"
  if [[ "$name" == ._* ]]; then
    continue
  fi
  ext="${name##*.}"
  ext_lower="$(printf "%s" "$ext" | tr '[:upper:]' '[:lower:]')"
  case "$ext_lower" in
    png|jpg|jpeg) ;;
    *) continue ;;
  esac
  stem="${name%.*}"
  family="$(family_stem_for "$stem")"
  if [[ -z "$family" || "$family" == */* ]]; then
    echo "invalid Neo Geo screenshot family '$family' from '$stem'" >&2
    exit 1
  fi
  source_images=$((source_images + 1))
  if [[ "$family" != "$stem" ]]; then
    collapsed_images=$((collapsed_images + 1))
  fi
  existing="$(existing_candidate_path "$family" || true)"
  if [[ -z "$existing" ]]; then
    cp "$file" "$CANDIDATES/$family.$ext_lower"
  elif [[ "$stem" == "$family" ]]; then
    remove_candidate_path "$family"
    cp "$file" "$CANDIDATES/$family.$ext_lower"
    parent_replacements=$((parent_replacements + 1))
  else
    duplicate_family_images=$((duplicate_family_images + 1))
  fi
done < <(find "$ORIGINALS" -maxdepth 1 -type f | LC_ALL=C sort)

staged_images="$(
  find "$CANDIDATES" -maxdepth 1 -type f \( -iname '*.png' -o -iname '*.jpg' -o -iname '*.jpeg' \) | wc -l | tr -d '[:space:]'
)"
if [[ "$staged_images" == "0" ]]; then
  echo "no staged Neo Geo screenshots in $CANDIDATES" >&2
  exit 1
fi
echo "neogeo_stage_summary source_images=$source_images staged_images=$staged_images collapsed_images=$collapsed_images duplicate_family_images=$duplicate_family_images parent_replacements=$parent_replacements family_lookup=$FAMILY_LOOKUP staged=$CANDIDATES"

if [[ "$STAGE_ONLY" == "1" ]]; then
  echo "neogeo_screenshot_pack stage_only=1 originals=$ORIGINALS staged=$CANDIDATES"
  exit 0
fi

echo "==> Building raw565 preview cache"
"$MISTER" preview-cache-build --input "$CANDIDATES" --output "$CACHE_DIR" --max "$MAX_SIZE"

if [[ "$DEPLOY" == "1" ]]; then
  remote_dir="$(dirname "$REMOTE_PACK")"
  echo "==> Deploying $PACK to $REMOTE_PACK"
  "$MISTER" run "mkdir -p '$remote_dir'"
  "$MISTER" put "$PACK" "$REMOTE_PACK"
fi

echo "neogeo_screenshot_pack originals=$ORIGINALS staged=$CANDIDATES raw565=$RAW_DIR pack=$PACK deploy=$DEPLOY"
