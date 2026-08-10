#!/usr/bin/env bash
# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

# Build a MiSTer SD-card-root distribution zip for MiSTer MagiK.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
DEFAULT_BIN="$ROOT/apps/mister/target/armv7-unknown-linux-gnueabihf/release-device/mister-magik-fb"
DEFAULT_INSTALLER="$ROOT/scripts/MiSTer-MagiK.sh"
DEFAULT_MANAGER="$ROOT/mister/tools/manager/target/armv7-unknown-linux-gnueabihf/release/mister-magik-manager"

BIN="$DEFAULT_BIN"
GAME_DATABASES_RELEASE_DIR=""
MAME_SQLITE=""
HBMAME_SQLITE=""
INSTALLER="$DEFAULT_INSTALLER"
MANAGER="$DEFAULT_MANAGER"
ASSET_PACK=""
SNES_ARTWORK="$ROOT/apps/mister/assets/snes/snes-small-v1.rgb565a"
SNES_ARTWORK_SHA256="7a76993e7e1b0063832b94e9d2ad588549587cf09a14ac2ced72d349ed12f766"
MAIN_BIN=""
MAIN_SOURCE_REVISION=""
SCANOUT_MODULE=""
SCANOUT_METADATA=""
LATCH_RBF=""
LATCH_METADATA=""
PLATFORM_MANIFEST=""
PLATFORM_BUNDLE_MANIFEST=""
GAME_DATABASES_MANIFEST=""
NAME="mister-magik"
OUT_DIR="$ROOT/dist"
VERSION=""
BUILD_NUMBER=""
RELEASE_ASSETS_DIR=""

usage() {
  sed -n '2,2p' "$0" | sed 's/^# \{0,1\}//'
  cat <<EOF

Usage:
  scripts/package-distribution.sh [options]

Options:
  --binary PATH        ARM mister-magik-fb binary.
                       Default: $DEFAULT_BIN
  --game-databases-release-dir PATH
                       Verified numbered release directory containing exactly
                       its archive, manifest, and SHA256SUMS (required).
  --installer PATH     MiSTer Scripts menu installer.
                       Default: $DEFAULT_INSTALLER
  --manager PATH       ARM installer lifecycle manager.
                       Default: $DEFAULT_MANAGER
  --asset-pack PATH    Optional preview asset pack. Build/publish packs from private/magik-cloud.
  --main-bin PATH      Required MiSTer_MagiK Main fork binary.
  --main-source-revision SHA
                       Source revision for --main-bin (required when it is supplied).
  --scanout-module PATH
                       Required qualified scanout module.
  --scanout-metadata PATH
                       Required scanout module metadata.
  --latch-rbf PATH     Required qualified production latch RBF.
  --latch-metadata PATH
                       Required production latch metadata.
  --platform-manifest PATH
                       Required canonical manifest matching every platform artifact.
  --platform-bundle-manifest PATH
                       Required durable platform bundle v0.1 or v0.2 manifest.
  --name NAME          Output basename. Default: mister-magik
  --out-dir PATH       Output directory. Default: dist
  --version VERSION    Required release version (0.2.BUILD).
  --build-number N     Required Info build number; must match VERSION and binary receipt.
  --release-assets-dir PATH
                       Optional output for flattened GitHub release assets and provenance.
  -h, --help           Show this help.

The zip is laid out relative to the MiSTer SD-card root:
  Scripts/MiSTer-MagiK.sh
  mister-magik/mister-magik-fb
  mister-magik/mister-magik-manager
  mister-magik/mame.sqlite3
  mister-magik/hbmame.sqlite3
  mister-magik/assets/...     when --asset-pack is provided
  MiSTer_MagiK
  mister-magik/platform-v3.manifest
  mister-magik/platform-bundle-v0.1.json or platform-bundle-v0.2.json
  mister-magik/game-databases-manifest.json
  mister-magik/mister_magik_scanout_slots.ko
  mister-magik/mister_magik_scanout_slots.metadata.txt
  mister-magik/fpga/menu-magik-vblank-latch.rbf
  mister-magik/fpga/menu-magik-vblank-latch.metadata.txt
  mister-magik/licenses/...   GPL, LGPL, OFL, CC BY-NC, and Rust notices
  mister-magik/THIRD-PARTY-NOTICES.txt
                              Metadata and bundled-component provenance
  mister-magik/SOURCE-OFFER.txt
                              Exact corresponding-source locations and revisions
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --binary)
      BIN="${2:?--binary requires a path}"
      shift 2
      ;;
    --game-databases-release-dir)
      GAME_DATABASES_RELEASE_DIR="${2:?--game-databases-release-dir requires a path}"
      shift 2
      ;;
    --installer)
      INSTALLER="${2:?--installer requires a path}"
      shift 2
      ;;
    --manager)
      MANAGER="${2:?--manager requires a path}"
      shift 2
      ;;
    --asset-pack)
      ASSET_PACK="${2:?--asset-pack requires a path}"
      shift 2
      ;;
    --main-bin)
      MAIN_BIN="${2:?--main-bin requires a path}"
      shift 2
      ;;
    --main-source-revision)
      MAIN_SOURCE_REVISION="${2:?--main-source-revision requires a revision}"
      shift 2
      ;;
    --scanout-module)
      SCANOUT_MODULE="${2:?--scanout-module requires a path}"
      shift 2
      ;;
    --scanout-metadata)
      SCANOUT_METADATA="${2:?--scanout-metadata requires a path}"
      shift 2
      ;;
    --latch-rbf)
      LATCH_RBF="${2:?--latch-rbf requires a path}"
      shift 2
      ;;
    --latch-metadata)
      LATCH_METADATA="${2:?--latch-metadata requires a path}"
      shift 2
      ;;
    --platform-manifest)
      PLATFORM_MANIFEST="${2:?--platform-manifest requires a path}"
      shift 2
      ;;
    --platform-bundle-manifest)
      PLATFORM_BUNDLE_MANIFEST="${2:?--platform-bundle-manifest requires a path}"
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
    --version)
      VERSION="${2:?--version requires a value}"
      shift 2
      ;;
    --build-number)
      BUILD_NUMBER="${2:?--build-number requires a value}"
      shift 2
      ;;
    --release-assets-dir)
      RELEASE_ASSETS_DIR="${2:?--release-assets-dir requires a path}"
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
if [[ -z "$GAME_DATABASES_RELEASE_DIR" || ! -d "$GAME_DATABASES_RELEASE_DIR" ]]; then
  echo "ERROR: --game-databases-release-dir is required and must be a directory." >&2
  exit 2
fi
DATABASE_TMP="$(mktemp -d "${TMPDIR:-/tmp}/mister-magik-game-databases.XXXXXX")"
trap 'rm -rf "$DATABASE_TMP"' EXIT
"$ROOT/scripts/agent" ci game-databases extract-release \
  "$GAME_DATABASES_RELEASE_DIR" --output "$DATABASE_TMP" >/dev/null
MAME_SQLITE="$DATABASE_TMP/mame.sqlite3"
HBMAME_SQLITE="$DATABASE_TMP/hbmame.sqlite3"
GAME_DATABASES_MANIFEST="$DATABASE_TMP/game-databases-manifest.json"
ARCADE_DATABASE_CSV="$DATABASE_TMP/ArcadeDatabase.csv"
ARCADE_DATABASE_LICENSE="$DATABASE_TMP/ArcadeDatabase-LICENSE.txt"
if [[ ! -f "$INSTALLER" ]]; then
  echo "ERROR: installer not found: $INSTALLER" >&2
  exit 1
fi
if grep -Eq 'mister-magik-agent|S00magik-agent|disabled-S00fastnet\.magik-agent' "$INSTALLER"; then
  echo "ERROR: public installer must not contain development-agent hook management." >&2
  exit 1
fi
if [[ ! "$BUILD_NUMBER" =~ ^[0-9]+$ || "$VERSION" != "0.2.$BUILD_NUMBER" ]]; then
  echo "ERROR: --version must equal 0.2.--build-number; got version=${VERSION:-missing} build=${BUILD_NUMBER:-missing}." >&2
  exit 2
fi
BIN_FEATURES="$(tr -d '\r\n' <"$BIN.features" 2>/dev/null || true)"
BIN_RECEIPT="$BIN.build-receipt.tsv"
receipt_field() {
  local key="$1"
  awk -F '\t' -v key="$key" 'NR == 1 { for (i = 2; i <= NF; i++) { if ($i ~ ("^" key "=")) { sub("^[^=]*=", "", $i); print $i; exit } } }' "$BIN_RECEIPT"
}
if [[ "$BIN_FEATURES" != "ui,profile" ]]; then
  echo "ERROR: production distribution requires the canonical ui,profile build; got ${BIN_FEATURES:-missing}." >&2
  exit 1
fi
if [[ ! -f "$BIN_RECEIPT" || "$(receipt_field build_number)" != "$BUILD_NUMBER" || "$(receipt_field version)" != "$VERSION" ]]; then
  echo "ERROR: binary build receipt does not match release version=$VERSION build=$BUILD_NUMBER." >&2
  exit 1
fi
MAGIK_SOURCE_REVISION="$(git -C "$ROOT" rev-parse HEAD)"
if [[ "$(receipt_field source_commit)" != "$MAGIK_SOURCE_REVISION" ]]; then
  echo "ERROR: binary receipt source revision does not match package checkout." >&2
  exit 1
fi
if [[ -n "$ASSET_PACK" && ! -f "$ASSET_PACK" ]]; then
  echo "ERROR: asset pack not found: $ASSET_PACK" >&2
  exit 1
fi
if [[ -z "$MAIN_BIN" || -z "$MAIN_SOURCE_REVISION" ]]; then
  echo "ERROR: --main-bin and --main-source-revision are required." >&2
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
for artifact in "$MAIN_BIN" "$SCANOUT_MODULE" "$SCANOUT_METADATA" "$LATCH_RBF" "$LATCH_METADATA" "$PLATFORM_MANIFEST" "$PLATFORM_BUNDLE_MANIFEST"; do
  if [[ -z "$artifact" || ! -f "$artifact" ]]; then
    echo "ERROR: required production platform artifact not found: ${artifact:-missing argument}" >&2
    exit 1
  fi
done
SCANOUT_SOURCE_REVISION="$(sed -n 's/^source_revision=//p' "$SCANOUT_METADATA")"
if [[ ! "$SCANOUT_SOURCE_REVISION" =~ ^[0-9a-f]{40}$ ]]; then
  echo "ERROR: scanout metadata lacks a valid source_revision." >&2
  exit 1
fi
LATCH_SOURCE_REVISION="$(sed -n 's/^source_commit=//p' "$LATCH_METADATA")"
if [[ ! "$LATCH_SOURCE_REVISION" =~ ^[0-9a-f]{40}$ ]]; then
  echo "ERROR: latch metadata lacks a valid source_commit." >&2
  exit 1
fi
manifest_field() {
  python3 - "$GAME_DATABASES_MANIFEST" "$1" <<'PY'
import json
import sys

value = json.load(open(sys.argv[1]))
for part in sys.argv[2].split("."):
    value = value[part]
print(value)
PY
}
GAME_DATABASE_VERSION="$(manifest_field release_version)"
MAME_SOURCE_REF="$(manifest_field sources.mame.tag)"
HBMAME_SOURCE_REVISION="$(manifest_field sources.hbmame.sha)"
ARCADE_DATABASE_SOURCE_REVISION="$(manifest_field sources.arcade_database.sha)"
if [[ "$(sed -n 's/^main_revision=//p' "$PLATFORM_MANIFEST")" != "$MAIN_SOURCE_REVISION" ]]; then
  echo "ERROR: --main-source-revision does not match platform manifest" >&2
  exit 1
fi
PLATFORM_BUNDLE_BASENAME="$(basename "$PLATFORM_BUNDLE_MANIFEST")"
PLATFORM_BUNDLE_ID="$(python3 - "$PLATFORM_BUNDLE_MANIFEST" "$PLATFORM_BUNDLE_BASENAME" <<'PY'
import json
import re
import sys

payload = json.load(open(sys.argv[1]))
formats = {
    "platform-bundle-v0.1.json": "mister-magik-platform-bundle-v0.1",
    "platform-bundle-v0.2.json": "mister-magik-platform-bundle-v0.2",
}
if formats.get(sys.argv[2]) != payload.get("format"):
    raise SystemExit("unsupported platform bundle manifest")
bundle_id = payload.get("bundle_id", "")
if not re.fullmatch(r"[0-9a-f]{64}", bundle_id):
    raise SystemExit("invalid platform bundle id")
print(bundle_id)
PY
)" || {
  echo "ERROR: --platform-bundle-manifest is invalid." >&2
  exit 1
}

mkdir -p "$OUT_DIR"
STAGE="$(mktemp -d "${TMPDIR:-/tmp}/mister-magik-dist.XXXXXX")"
trap 'rm -rf "$STAGE" "$DATABASE_TMP"' EXIT

mkdir -p "$STAGE/Scripts" "$STAGE/mister-magik/fpga" "$STAGE/mister-magik/licenses"
cp "$INSTALLER" "$STAGE/Scripts/MiSTer-MagiK.sh"
chmod 755 "$STAGE/Scripts/MiSTer-MagiK.sh"
cp "$BIN" "$STAGE/mister-magik/mister-magik-fb"
chmod 755 "$STAGE/mister-magik/mister-magik-fb"
cp "$MANAGER" "$STAGE/mister-magik/mister-magik-manager"
chmod 755 "$STAGE/mister-magik/mister-magik-manager"
cp "$MAME_SQLITE" "$STAGE/mister-magik/mame.sqlite3"
if [[ -n "$HBMAME_SQLITE" ]]; then
  cp "$HBMAME_SQLITE" "$STAGE/mister-magik/hbmame.sqlite3"
fi

ACTUAL_SNES_ARTWORK_SHA256="$(python3 -c 'import hashlib,sys; print(hashlib.sha256(open(sys.argv[1], "rb").read()).hexdigest())' "$SNES_ARTWORK")"
if [[ "$ACTUAL_SNES_ARTWORK_SHA256" != "$SNES_ARTWORK_SHA256" ]]; then
  echo "ERROR: SNES artwork checksum mismatch." >&2
  exit 1
fi
mkdir -p "$STAGE/mister-magik/assets/snes"
cp "$SNES_ARTWORK" "$STAGE/mister-magik/assets/snes/snes-small-v1.rgb565a"

if [[ -n "$ASSET_PACK" ]]; then
  mkdir -p "$STAGE/mister-magik/assets"
  cp "$ASSET_PACK" "$STAGE/mister-magik/assets/$(basename "$ASSET_PACK")"
fi

cp "$MAIN_BIN" "$STAGE/MiSTer_MagiK"
cp "$SCANOUT_MODULE" "$STAGE/mister-magik/mister_magik_scanout_slots.ko"
cp "$SCANOUT_METADATA" "$STAGE/mister-magik/mister_magik_scanout_slots.metadata.txt"
cp "$LATCH_RBF" "$STAGE/mister-magik/fpga/menu-magik-vblank-latch.rbf"
cp "$LATCH_METADATA" "$STAGE/mister-magik/fpga/menu-magik-vblank-latch.metadata.txt"
cp "$PLATFORM_MANIFEST" "$STAGE/mister-magik/platform-v3.manifest"
cp "$PLATFORM_BUNDLE_MANIFEST" "$STAGE/mister-magik/$PLATFORM_BUNDLE_BASENAME"
cp "$GAME_DATABASES_MANIFEST" "$STAGE/mister-magik/game-databases-manifest.json"
MAIN_SHA256="$(python3 -c 'import hashlib,sys; print(hashlib.sha256(open(sys.argv[1], "rb").read()).hexdigest())' "$MAIN_BIN")"
cat >"$STAGE/mister-magik/release-v1.txt" <<EOF
format=mister-magik-release-v1
version=$VERSION
build_number=$BUILD_NUMBER
magik_revision=$MAGIK_SOURCE_REVISION
main_revision=$MAIN_SOURCE_REVISION
main_sha256=$MAIN_SHA256
features=$BIN_FEATURES
platform_bundle_id=$PLATFORM_BUNDLE_ID
game_database_version=$GAME_DATABASE_VERSION
EOF
chmod 755 "$STAGE/MiSTer_MagiK"
"$ROOT/scripts/agent" ci platform-manifest verify \
  "$STAGE/mister-magik/platform-v3.manifest" --root "$STAGE" --layout public >/dev/null
if find "$STAGE" -type f \( -path '*/experiments/*' -o -name menu.rbf \) -print -quit | grep -q .; then
  echo "ERROR: production package contains experiments/ or root menu.rbf" >&2
  exit 1
fi

# Keep every binary distribution self-describing. These are copied rather than
# merely linked so an extracted SD-card package retains the notices.
cp "$ROOT/LICENSE" "$STAGE/mister-magik/licenses/MiSTer-MagiK-GPL-3.0-or-later.txt"
cp "$ROOT/apps/mister/licenses/RUST-LIBRARIES.txt" "$STAGE/mister-magik/licenses/RUST-LIBRARIES.txt"
cp "$ROOT/apps/mister/licenses/FFMPEG.txt" "$STAGE/mister-magik/licenses/FFMPEG-LGPL-2.1-or-later.txt"
cp "$ROOT/apps/mister/licenses/PRESS-START-2P.txt" "$STAGE/mister-magik/licenses/PRESS-START-2P-OFL-1.1.txt"
cp "$ROOT/apps/mister/licenses/COMMERCIAL-FONTS.txt" "$STAGE/mister-magik/licenses/COMMERCIAL-FONTS.txt"
cp "$ROOT/crates/particles/assets/cabinet/arcade-cabinet.LICENSE.txt" \
  "$STAGE/mister-magik/licenses/ARCADE-CABINET-CC-BY-NC-4.0.txt"
mkdir -p "$STAGE/mister-magik/licenses/ArcadeDatabase_MiSTer"
cp "$ARCADE_DATABASE_CSV" "$STAGE/mister-magik/licenses/ArcadeDatabase_MiSTer/ArcadeDatabase.csv"
cp "$ARCADE_DATABASE_LICENSE" "$STAGE/mister-magik/licenses/ArcadeDatabase_MiSTer/LICENSE.txt"
cat > "$STAGE/mister-magik/THIRD-PARTY-NOTICES.txt" <<EOF
MiSTer MagiK distribution notices
==================================

Copyright (C) 2026 Nigel Breslaw

MiSTer MagiK is GPL-3.0-or-later. Its full license is in
mister-magik/licenses/MiSTer-MagiK-GPL-3.0-or-later.txt.

The first-party mister_magik_scanout_slots kernel module is also
GPL-3.0-or-later. Corresponding source is included in the MiSTer MagiK source
revision named by SOURCE-OFFER.txt.
Its Linux loader classification is a compatibility marker, not its source
license; see the module metadata and corresponding source in this package.

The FPGA latch RBF combines MiSTer MagiK latch code with Menu_MiSTer source at
the exact upstream revision named by SOURCE-OFFER.txt. The Menu_MiSTer-derived
portion remains under its upstream GPL-3.0 terms.

The launcher includes Slint under its GPL-3.0-only option, the normal runtime
Rust dependency closure, statically linked FFmpeg libraries under LGPL-2.1-or-later,
the Press Start 2P and Jersey 25 fonts under SIL OFL-1.1, commercially licensed
Yesterday 10, Xerxes 10, and Nocive 15 bitmap glyphs, and the Arcade Cabinet particle model
by Lluc Guardiolaa under CC-BY-NC-4.0. Complete notices and attribution are in
the mister-magik/licenses/ directory.

mame.sqlite3 is generated metadata, not ROM, BIOS, firmware, or game media. It
is derived from MAME listxml and software-list data from mamedev/mame at ref:
  $MAME_SOURCE_REF
MAME is distributed under the BSD 3-Clause License. Source and license:
  https://github.com/mamedev/mame/tree/$MAME_SOURCE_REF

The arcade-specific rows embedded in mame.sqlite3 are derived from
MiSTer-devel/ArcadeDatabase_MiSTer at revision:
  $ARCADE_DATABASE_SOURCE_REVISION
The exact CSV and GPL-3.0 license are included under:
  mister-magik/licenses/ArcadeDatabase_MiSTer/
EOF
if [[ -n "$HBMAME_SQLITE" ]]; then
  cat >> "$STAGE/mister-magik/THIRD-PARTY-NOTICES.txt" <<EOF

hbmame.sqlite3 is generated metadata, not ROM, BIOS, firmware, or game media.
It is derived from HBMAME listxml at revision:
  $HBMAME_SOURCE_REVISION
HBMAME source and license:
  https://github.com/Robbbert/hbmame/tree/$HBMAME_SOURCE_REVISION
EOF
fi
cat > "$STAGE/mister-magik/SOURCE-OFFER.txt" <<EOF
Corresponding source and relinking instructions
===============================================

MiSTer MagiK source (including build and installation scripts):
  https://github.com/NigelBreslaw/MiSTer-MagiK/tree/$(git -C "$ROOT" rev-parse HEAD)

MiSTer MagiK scanout kernel-module source matching the shipped module:
  https://github.com/NigelBreslaw/MiSTer-MagiK/tree/$SCANOUT_SOURCE_REVISION/mister/platform/kernel/scanout-slots

Menu_MiSTer source matching the shipped FPGA latch RBF:
  https://github.com/MiSTer-devel/Menu_MiSTer/tree/$LATCH_SOURCE_REVISION
The MiSTer MagiK patch applied to that source is:
  https://github.com/NigelBreslaw/MiSTer-MagiK/tree/$(git -C "$ROOT" rev-parse HEAD)/mister/platform/fpga/menu-vblank-latch

FFmpeg 8.1.2 source, used by the production UI build:
  https://github.com/FFmpeg/FFmpeg/tree/n8.1.2
The exact configure flags and cross-build procedure are in:
  agent-cli/src/build.rs
at the MiSTer MagiK source revision above.
The MiSTer MagiK source, Cargo.lock, and build scripts are the complete source
needed to rebuild the application and relink it with a modified FFmpeg build.
EOF
cat >> "$STAGE/mister-magik/SOURCE-OFFER.txt" <<EOF

MiSTer_MagiK Main fork source:
  https://github.com/NigelBreslaw/Main_MiSTer/tree/$MAIN_SOURCE_REVISION
EOF

OUT="$OUT_DIR/$NAME.zip"
rm -f "$OUT"
(
  cd "$STAGE"
  zip -qr "$OUT" .
)

if [[ -n "$RELEASE_ASSETS_DIR" ]]; then
  python3 "$ROOT/scripts/release/packaging/package-release-assets.py" \
    --stage "$STAGE" \
    --zip "$OUT" \
    --output "$RELEASE_ASSETS_DIR" \
    --version "$VERSION" \
    --build-number "$BUILD_NUMBER"
fi

echo "$OUT"
