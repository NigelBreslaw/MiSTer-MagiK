#!/usr/bin/env bash
# Build a MiSTer SD-card-root distribution zip for MiSTer MagiK.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
DEFAULT_BIN="$ROOT/magik-gui/target/armv7-unknown-linux-gnueabihf/release-device/mister-magik-fb"
DEFAULT_MAME="$ROOT/build/mame.sqlite3"
DEFAULT_HBMAME="$ROOT/build/hbmame.sqlite3"
DEFAULT_INSTALLER="$ROOT/scripts/mister-magik.sh"

BIN="$DEFAULT_BIN"
MAME_SQLITE="$DEFAULT_MAME"
HBMAME_SQLITE=""
INSTALLER="$DEFAULT_INSTALLER"
ASSET_PACK=""
MAIN_BIN=""
NAME="mister-magik"
OUT_DIR="$ROOT/dist"

usage() {
  sed -n '2,2p' "$0" | sed 's/^# \{0,1\}//'
  cat <<EOF

Usage:
  scripts/package-distribution.sh [options]

Options:
  --binary PATH        ARM mister-magik-fb binary.
                       Default: $DEFAULT_BIN
  --mame-sqlite PATH   MAME metadata SQLite database.
                       Default: $DEFAULT_MAME
  --hbmame-sqlite PATH Optional HBMame metadata SQLite database.
                       Default if --hbmame-sqlite-default: $DEFAULT_HBMAME
  --installer PATH     MiSTer Scripts menu installer.
                       Default: $DEFAULT_INSTALLER
  --asset-pack PATH    Optional preview asset pack. Build/publish packs from ../magik-cloud.
  --hbmame-sqlite-default
                       Include the default HBMame metadata DB if present.
  --main-bin PATH      Optional MiSTer_MagiK Main fork binary.
  --name NAME          Output basename. Default: mister-magik
  --out-dir PATH       Output directory. Default: dist
  -h, --help           Show this help.

The zip is laid out relative to the MiSTer SD-card root:
  Scripts/mister-magik.sh
  mister-magik/mister-magik-fb
  mister-magik/mame.sqlite3
  mister-magik/hbmame.sqlite3   when --hbmame-sqlite is provided
  mister-magik/assets/...     when --asset-pack is provided
  MiSTer_MagiK                when --main-bin is provided
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --binary)
      BIN="${2:?--binary requires a path}"
      shift 2
      ;;
    --mame-sqlite)
      MAME_SQLITE="${2:?--mame-sqlite requires a path}"
      shift 2
      ;;
    --hbmame-sqlite)
      HBMAME_SQLITE="${2:?--hbmame-sqlite requires a path}"
      shift 2
      ;;
    --installer)
      INSTALLER="${2:?--installer requires a path}"
      shift 2
      ;;
    --asset-pack)
      ASSET_PACK="${2:?--asset-pack requires a path}"
      shift 2
      ;;
    --hbmame-sqlite-default)
      HBMAME_SQLITE="$DEFAULT_HBMAME"
      shift
      ;;
    --main-bin)
      MAIN_BIN="${2:?--main-bin requires a path}"
      shift 2
      ;;
    --name)
      NAME="${2:?--name requires a basename}"
      shift 2
      ;;
    --out-dir)
      OUT_DIR="${2:?--out-dir requires a path}"
      shift 2
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      echo "ERROR: unknown argument: $1" >&2
      exit 2
      ;;
  esac
done

if ! command -v zip >/dev/null 2>&1; then
  echo "ERROR: zip is required." >&2
  exit 1
fi
if [[ ! -f "$BIN" ]]; then
  echo "ERROR: binary not found: $BIN" >&2
  exit 1
fi
if [[ ! -f "$MAME_SQLITE" ]]; then
  echo "ERROR: MAME metadata DB not found: $MAME_SQLITE" >&2
  echo "       Build it with: scripts/mister mame-metadata-build --out '$MAME_SQLITE'" >&2
  exit 1
fi
if [[ ! -f "$INSTALLER" ]]; then
  echo "ERROR: installer not found: $INSTALLER" >&2
  exit 1
fi
if [[ -n "$ASSET_PACK" && ! -f "$ASSET_PACK" ]]; then
  echo "ERROR: asset pack not found: $ASSET_PACK" >&2
  exit 1
fi
if [[ -n "$HBMAME_SQLITE" && ! -f "$HBMAME_SQLITE" ]]; then
  echo "ERROR: HBMame metadata DB not found: $HBMAME_SQLITE" >&2
  echo "       Build it with: scripts/mister mame-metadata-build --out '$HBMAME_SQLITE' --mame /path/to/hbmame" >&2
  exit 1
fi
if [[ -n "$MAIN_BIN" && ! -f "$MAIN_BIN" ]]; then
  echo "ERROR: Main fork binary not found: $MAIN_BIN" >&2
  exit 1
fi

mkdir -p "$OUT_DIR"
STAGE="$(mktemp -d "${TMPDIR:-/tmp}/mister-magik-dist.XXXXXX")"
trap 'rm -rf "$STAGE"' EXIT

mkdir -p "$STAGE/Scripts" "$STAGE/mister-magik"
cp "$INSTALLER" "$STAGE/Scripts/mister-magik.sh"
chmod 755 "$STAGE/Scripts/mister-magik.sh"
cp "$BIN" "$STAGE/mister-magik/mister-magik-fb"
chmod 755 "$STAGE/mister-magik/mister-magik-fb"
cp "$MAME_SQLITE" "$STAGE/mister-magik/mame.sqlite3"
if [[ -n "$HBMAME_SQLITE" ]]; then
  cp "$HBMAME_SQLITE" "$STAGE/mister-magik/hbmame.sqlite3"
fi

if [[ -n "$ASSET_PACK" ]]; then
  mkdir -p "$STAGE/mister-magik/assets"
  cp "$ASSET_PACK" "$STAGE/mister-magik/assets/$(basename "$ASSET_PACK")"
fi

if [[ -n "$MAIN_BIN" ]]; then
  cp "$MAIN_BIN" "$STAGE/MiSTer_MagiK"
  chmod 755 "$STAGE/MiSTer_MagiK"
fi

OUT="$OUT_DIR/$NAME.zip"
rm -f "$OUT"
(
  cd "$STAGE"
  zip -qr "$OUT" .
)

echo "$OUT"
