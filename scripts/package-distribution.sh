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
MAIN_SOURCE_REVISION=""
MAME_SOURCE_REF=""
HBMAME_SOURCE_REVISION=""
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
  --asset-pack PATH    Optional preview asset pack. Build/publish packs from private/magik-cloud.
  --hbmame-sqlite-default
                       Include the default HBMame metadata DB if present.
  --main-bin PATH      Optional MiSTer_MagiK Main fork binary.
  --main-source-revision SHA
                       Source revision for --main-bin (required when it is supplied).
  --mame-source-ref REF
                       mamedev/mame ref used to build --mame-sqlite (required).
  --hbmame-source-revision SHA
                       Robbbert/hbmame revision used to build --hbmame-sqlite (required
                       when it is supplied).
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
  licenses/...                GPL, LGPL, OFL, and Rust dependency notices
  THIRD-PARTY-NOTICES.txt     Metadata and bundled-component provenance
  SOURCE-OFFER.txt            Exact corresponding-source locations and revisions
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
    --main-source-revision)
      MAIN_SOURCE_REVISION="${2:?--main-source-revision requires a revision}"
      shift 2
      ;;
    --mame-source-ref)
      MAME_SOURCE_REF="${2:?--mame-source-ref requires a ref}"
      shift 2
      ;;
    --hbmame-source-revision)
      HBMAME_SOURCE_REVISION="${2:?--hbmame-source-revision requires a revision}"
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
  echo "       Build it with: scripts/mister mame-metadata-build --out '$MAME_SQLITE' [--category-ini /path/to/catver.ini]" >&2
  exit 1
fi
if [[ ! -f "$INSTALLER" ]]; then
  echo "ERROR: installer not found: $INSTALLER" >&2
  exit 1
fi
if [[ -z "$MAME_SOURCE_REF" ]]; then
  echo "ERROR: --mame-source-ref is required so the distributed metadata has reproducible provenance." >&2
  exit 2
fi
if [[ -n "$ASSET_PACK" && ! -f "$ASSET_PACK" ]]; then
  echo "ERROR: asset pack not found: $ASSET_PACK" >&2
  exit 1
fi
if [[ -n "$HBMAME_SQLITE" && ! -f "$HBMAME_SQLITE" ]]; then
  echo "ERROR: HBMame metadata DB not found: $HBMAME_SQLITE" >&2
  echo "       Build it with: scripts/mister mame-metadata-build --out '$HBMAME_SQLITE' --mame /path/to/hbmame [--category-ini /path/to/catver.ini]" >&2
  exit 1
fi
if [[ -n "$HBMAME_SQLITE" && -z "$HBMAME_SOURCE_REVISION" ]]; then
  echo "ERROR: --hbmame-source-revision is required with --hbmame-sqlite." >&2
  exit 2
fi
if [[ -n "$MAIN_BIN" && -z "$MAIN_SOURCE_REVISION" ]]; then
  echo "ERROR: --main-source-revision is required with --main-bin." >&2
  exit 2
fi
if [[ -n "$HBMAME_SQLITE" ]]; then
  hbmame_bytes="$(stat -f%z "$HBMAME_SQLITE" 2>/dev/null || stat -c%s "$HBMAME_SQLITE")"
  if [[ "$hbmame_bytes" -lt 1048576 ]]; then
    echo "ERROR: HBMame metadata DB is suspiciously small: $HBMAME_SQLITE ($hbmame_bytes bytes)" >&2
    exit 1
  fi
  if command -v sqlite3 >/dev/null 2>&1; then
    hbmame_rows="$(sqlite3 "$HBMAME_SQLITE" "SELECT count(*) FROM mame_machines;" 2>/dev/null || true)"
    if [[ "${hbmame_rows:-0}" -lt 5000 ]]; then
      echo "ERROR: HBMame metadata DB has too few machine rows: ${hbmame_rows:-unreadable}" >&2
      exit 1
    fi
    marpy_parent="$(sqlite3 "$HBMAME_SQLITE" "SELECT COALESCE(parent_setname, '') FROM mame_machines WHERE setname = 'marpy';" 2>/dev/null || true)"
    if [[ "$marpy_parent" != "mappy" ]]; then
      echo "ERROR: HBMame metadata sentinel failed: expected marpy parent mappy, got '${marpy_parent:-missing}'" >&2
      exit 1
    fi
  fi
fi
if [[ -n "$MAIN_BIN" && ! -f "$MAIN_BIN" ]]; then
  echo "ERROR: Main fork binary not found: $MAIN_BIN" >&2
  exit 1
fi

mkdir -p "$OUT_DIR"
STAGE="$(mktemp -d "${TMPDIR:-/tmp}/mister-magik-dist.XXXXXX")"
trap 'rm -rf "$STAGE"' EXIT

mkdir -p "$STAGE/Scripts" "$STAGE/mister-magik/art" "$STAGE/licenses"
cp "$INSTALLER" "$STAGE/Scripts/mister-magik.sh"
chmod 755 "$STAGE/Scripts/mister-magik.sh"
cp "$BIN" "$STAGE/mister-magik/mister-magik-fb"
chmod 755 "$STAGE/mister-magik/mister-magik-fb"
python3 "$ROOT/scripts/png-to-slint-rgba.py" \
  "$ROOT/magik-gui/ui/art/arcade-cabinet-preview.png" \
  "$STAGE/mister-magik/art/arcade-cabinet-preview.rgba"
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

# Keep every binary distribution self-describing. These are copied rather than
# merely linked so an extracted SD-card package retains the notices.
cp "$ROOT/LICENSE" "$STAGE/licenses/MiSTer-MagiK-GPL-3.0-or-later.txt"
cp "$ROOT/magik-gui/licenses/RUST-LIBRARIES.txt" "$STAGE/licenses/RUST-LIBRARIES.txt"
cp "$ROOT/magik-gui/licenses/FFMPEG.txt" "$STAGE/licenses/FFMPEG-LGPL-2.1-or-later.txt"
cp "$ROOT/magik-gui/licenses/PRESS-START-2P.txt" "$STAGE/licenses/PRESS-START-2P-OFL-1.1.txt"
cat > "$STAGE/THIRD-PARTY-NOTICES.txt" <<EOF
MiSTer MagiK distribution notices
==================================

MiSTer MagiK is GPL-3.0-or-later. Its full license is in
licenses/MiSTer-MagiK-GPL-3.0-or-later.txt.

The launcher includes Slint under its GPL-3.0-only option, the normal runtime
Rust dependency closure, statically linked FFmpeg libraries under LGPL-2.1-or-later,
and the Press Start 2P font under SIL OFL-1.1. Their complete notices are in
the licenses/ directory.

mame.sqlite3 is generated metadata, not ROM, BIOS, firmware, or game media. It
is derived from MAME listxml and software-list data from mamedev/mame at ref:
  $MAME_SOURCE_REF
MAME is distributed under the BSD 3-Clause License. Source and license:
  https://github.com/mamedev/mame/tree/$MAME_SOURCE_REF
EOF
if [[ -n "$HBMAME_SQLITE" ]]; then
  cat >> "$STAGE/THIRD-PARTY-NOTICES.txt" <<EOF

hbmame.sqlite3 is generated metadata, not ROM, BIOS, firmware, or game media.
It is derived from HBMAME listxml at revision:
  $HBMAME_SOURCE_REVISION
HBMAME source and license:
  https://github.com/Robbbert/hbmame/tree/$HBMAME_SOURCE_REVISION
EOF
fi
cat > "$STAGE/SOURCE-OFFER.txt" <<EOF
Corresponding source and relinking instructions
===============================================

MiSTer MagiK source (including build and installation scripts):
  https://github.com/NigelBreslaw/MiSTer-MagiK/tree/$(git -C "$ROOT" rev-parse HEAD)

FFmpeg 8.1.2 source, used by the optional video build:
  https://github.com/FFmpeg/FFmpeg/tree/n8.1.2
The exact configure flags and cross-build procedure are in:
  magik-gui/scripts/build-minimal-ffmpeg.sh
at the MiSTer MagiK source revision above.
The MiSTer MagiK source, Cargo.lock, and build scripts are the complete source
needed to rebuild the application and relink it with a modified FFmpeg build.
EOF
if [[ -n "$MAIN_BIN" ]]; then
  cat >> "$STAGE/SOURCE-OFFER.txt" <<EOF

MiSTer_MagiK Main fork source:
  https://github.com/NigelBreslaw/Main_MiSTer/tree/$MAIN_SOURCE_REVISION
EOF
fi

OUT="$OUT_DIR/$NAME.zip"
rm -f "$OUT"
(
  cd "$STAGE"
  zip -qr "$OUT" .
)

echo "$OUT"
