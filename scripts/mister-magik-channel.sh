#!/bin/sh
# Select the MiSTer MagiK Downloader/update_all channel without touching boot state.
set -eu

FAT="${MISTER_MAGIK_FAT:-/media/fat}"
DROP_IN="$FAT/downloader_mister_magik.ini"
BETA_URL="https://raw.githubusercontent.com/NigelBreslaw/MiSTer-MagiK/downloader/mister-magik-beta-db.json.zip"
RELEASE_URL="https://raw.githubusercontent.com/NigelBreslaw/MiSTer-MagiK/downloader/mister-magik-release-db.json.zip"

channel="${1:-}"
if [ -z "$channel" ]; then
  printf '%s\n' "MiSTer MagiK update channel" "  1) Beta" "  2) Release"
  printf 'Select [1-2]: '
  read -r answer
  case "$answer" in
    1) channel=beta ;;
    2) channel=release ;;
    *) echo "No change."; exit 1 ;;
  esac
fi

case "$channel" in
  beta) url="$BETA_URL" ;;
  release) url="$RELEASE_URL" ;;
  *) echo "Usage: $0 [beta|release]" >&2; exit 2 ;;
esac

mkdir -p "$FAT"
tmp="$DROP_IN.tmp.$$"
trap 'rm -f "$tmp"' EXIT HUP INT TERM
cat >"$tmp" <<EOF
[mister_magik]
db_url = $url
EOF
mv "$tmp" "$DROP_IN"
trap - EXIT HUP INT TERM
echo "MiSTer MagiK update channel: $channel"
echo "Run update_all to apply updates from this channel."

