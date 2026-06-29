#!/usr/bin/env bash
# Sync half-resolution MP4 video snaps to the MiSTer without re-encoding them.
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
MISTER="$HERE/scripts/mister"

SRC_DIR="${1:-${MISTER_VIDEO_SRC_DIR:-$HERE/build/video-snaps-neogeo-halfres}}"
REMOTE_DIR="${2:-${MISTER_VIDEO_REMOTE_DIR:-/media/fat/mister-magik/video-snaps/neogeo}}"

if [[ ! -d "$SRC_DIR" ]]; then
  echo "sync-video-snaps: source directory not found: $SRC_DIR" >&2
  exit 1
fi

shopt -s nullglob
files=("$SRC_DIR"/*.mp4 "$SRC_DIR"/*.MP4)
shopt -u nullglob

if [[ "${#files[@]}" -eq 0 ]]; then
  echo "sync-video-snaps: no .mp4 files in $SRC_DIR" >&2
  exit 1
fi

IFS=$'\n' files=($(printf '%s\n' "${files[@]}" | sort -f))
unset IFS

"$MISTER" run "mkdir -p '$REMOTE_DIR'; rm -f '$REMOTE_DIR'/*.mp4 '$REMOTE_DIR'/*.MP4"
for file in "${files[@]}"; do
  base="$(basename "$file")"
  echo "sync-video-snaps: $base"
  "$MISTER" put "$file" "$REMOTE_DIR/$base"
done

echo "sync-video-snaps: synced ${#files[@]} file(s) to $REMOTE_DIR"
