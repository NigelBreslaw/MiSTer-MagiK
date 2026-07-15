#!/usr/bin/env bash
# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

# Public-beta host release gate for MiSTer MagiK.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BIN="$ROOT/magik-gui/target/armv7-unknown-linux-gnueabihf/release-device/mister-magik-fb"
CATALOG_BUILDER="$ROOT/magik-gui/target/armv7-unknown-linux-gnueabihf/release-device/mister-magik-catalog-builder"
WORK="$ROOT/build/release-check-host"
MAIN_BIN="${MISTER_MAIN_BIN:-$ROOT/../Main_MiSTer/bin/MiSTer}"

usage() {
  cat <<'EOF'
usage: scripts/release-check-host.sh

Runs the host/build side of the public-beta release gate:
  - Rust formatting
  - host logic tests
  - catalog crate tests
  - host tool tests
  - clippy for app host logic, catalog, tools, and agent
  - ARM release-device build
  - ARM shared-library check
  - distribution package dry-run and zip layout validation

Set MISTER_MAIN_BIN=/path/to/MiSTer to include a Main_MiSTer fork binary in the
package dry-run. If the default sibling checkout binary exists, it is included.
EOF
}

if [ "${1:-}" = "-h" ] || [ "${1:-}" = "--help" ]; then
  usage
  exit 0
fi
if [ "$#" -gt 0 ]; then
  echo "ERROR: unknown argument: $1" >&2
  usage >&2
  exit 2
fi

step() {
  echo
  echo "==> $*"
}

step "Rust format"
"$ROOT/scripts/dev-rust" fmt

step "Host logic tests"
"$ROOT/scripts/dev-rust" test

step "Catalog crate tests"
cargo test --manifest-path "$ROOT/magik-gui/catalog/Cargo.toml"

step "Host tool tests"
"$ROOT/scripts/dev-rust" host-tools

step "Clippy magik-gui host logic"
(
  cd "$ROOT/magik-gui"
  cargo clippy --lib --no-default-features -- -D warnings
)

step "Clippy catalog crate"
cargo clippy --manifest-path "$ROOT/magik-gui/catalog/Cargo.toml" --all-targets -- -D warnings

step "Clippy host tools"
cargo clippy --manifest-path "$ROOT/tools/mister/Cargo.toml" --all-targets -- -D warnings

step "Clippy MagiK agent"
cargo clippy --manifest-path "$ROOT/tools/magik-agent/Cargo.toml" --all-targets -- -D warnings

step "ARM release-device ui,video build"
"$ROOT/magik-gui/build-arm.sh" --device --video

step "ARM catalog builder build"
"$ROOT/scripts/build-catalog-builder.sh" --device

step "ARM shared-library check"
"$ROOT/magik-gui/scripts/check-arm-shared-libs.sh" "$BIN"

step "Distribution package dry-run"
rm -rf "$WORK"
mkdir -p "$WORK"
MAME_SQLITE="$WORK/mame.sqlite3"
HBMAME_SQLITE="$WORK/hbmame.sqlite3"
python3 - "$MAME_SQLITE" "$HBMAME_SQLITE" <<'PY'
import sqlite3
import sys

mame = sqlite3.connect(sys.argv[1])
mame.executescript("""
CREATE TABLE mame_machines(setname TEXT PRIMARY KEY,parent_setname TEXT,title TEXT NOT NULL,source_version TEXT NOT NULL) WITHOUT ROWID;
WITH RECURSIVE seq(i) AS (VALUES(1) UNION ALL SELECT i+1 FROM seq WHERE i<50000)
INSERT INTO mame_machines SELECT 'machine'||i,'','Machine '||i,'0.288 (mame0288)' FROM seq;
CREATE TABLE mame_software_items(list_name TEXT NOT NULL,item_name TEXT NOT NULL);
INSERT INTO mame_software_items VALUES('megadriv','one'),('n64','one'),('nes','one'),('saturn','one'),('sms','one'),('snes','one');
""")
mame.commit()
mame.close()
hbmame = sqlite3.connect(sys.argv[2])
hbmame.executescript("""
CREATE TABLE mame_machines(setname TEXT PRIMARY KEY,parent_setname TEXT,title TEXT NOT NULL) WITHOUT ROWID;
WITH RECURSIVE seq(i) AS (VALUES(1) UNION ALL SELECT i+1 FROM seq WHERE i<5000)
INSERT INTO mame_machines SELECT 'machine'||i,'','Machine '||i FROM seq;
INSERT INTO mame_machines VALUES('marpy','mappy','Marpy');
CREATE TABLE package_padding(data BLOB NOT NULL);
INSERT INTO package_padding VALUES(zeroblob(1048576));
""")
hbmame.commit()
hbmame.close()
PY
"$ROOT/scripts/game-databases-bundle.py" create \
  --mame-sqlite "$MAME_SQLITE" --hbmame-sqlite "$HBMAME_SQLITE" \
  --release-version 1 --mame-tag mame0288 \
  --mame-sha 1111111111111111111111111111111111111111 \
  --mame-listxml-asset mame0288lx.zip \
  --mame-listxml-sha256 "$(printf 5%.0s {1..64})" --hbmame-tag tag24532 \
  --hbmame-sha 2222222222222222222222222222222222222222 \
  --mame-builder-sha "$(git -C "$ROOT" rev-parse HEAD)" \
  --hbmame-builder-sha "$(git -C "$ROOT" rev-parse HEAD)" \
  --output "$WORK/game-databases" >/dev/null

package_args=(
  --binary "$BIN"
  --mame-sqlite "$MAME_SQLITE"
  --hbmame-sqlite "$HBMAME_SQLITE"
  --game-databases-manifest "$WORK/game-databases/game-databases-manifest.json"
  --name release-check
  --out-dir "$WORK"
  --version "$(source "$ROOT/scripts/bench-context-lib.sh"; bench_context_build_receipt_field "$BIN" version)"
  --build-number "$(source "$ROOT/scripts/bench-context-lib.sh"; bench_context_build_receipt_field "$BIN" build_number)"
  --release-assets-dir "$WORK/release-assets"
)
if [ -f "$MAIN_BIN" ]; then
  MAIN_SOURCE_REVISION="$(git -C "$(dirname "$MAIN_BIN")/.." rev-parse HEAD)"
else
  MAIN_BIN="$WORK/MiSTer_MagiK"
  cp "$BIN" "$MAIN_BIN"
  MAIN_SOURCE_REVISION="1111111111111111111111111111111111111111"
fi
MAGIK_REVISION="$(git -C "$ROOT" rev-parse HEAD)"
MENU_REVISION="3333333333333333333333333333333333333333"
printf 'module release-check\n' > "$WORK/mister_magik_scanout_slots.ko"
printf 'rbf release-check\n' > "$WORK/menu-magik-vblank-latch.rbf"
CONTRACT="$(printf release-check-contract | sha256sum | awk '{print $1}')"
printf 'platform_contract_sha256=%s\nmodule_sha256=%s\nvermagic=5.15.1-MiSTer fixture\nsource_revision=%s\n' \
  "$CONTRACT" "$(sha256sum "$WORK/mister_magik_scanout_slots.ko" | awk '{print $1}')" "$MAGIK_REVISION" > "$WORK/scanout.metadata.txt"
printf 'format=mister-magik-fpga-release-v1\nplatform_contract_sha256=%s\nmagik_commit=%s\nsource_commit=%s\nrbf_sha256=%s\n' \
  "$CONTRACT" "$MAGIK_REVISION" "$MENU_REVISION" \
  "$(sha256sum "$WORK/menu-magik-vblank-latch.rbf" | awk '{print $1}')" > "$WORK/latch.metadata.txt"
"$ROOT/scripts/platform-manifest.py" generate \
  --output "$WORK/platform-v1.manifest" --main "$MAIN_BIN" --gui "$BIN" \
  --catalog-builder "$CATALOG_BUILDER" \
  --scanout-module "$WORK/mister_magik_scanout_slots.ko" --scanout-metadata "$WORK/scanout.metadata.txt" \
  --latch-rbf "$WORK/menu-magik-vblank-latch.rbf" --latch-metadata "$WORK/latch.metadata.txt" \
  --main-revision "$MAIN_SOURCE_REVISION" --magik-revision "$MAGIK_REVISION" >/dev/null
printf '{"format":"mister-magik-platform-bundle-v0.1","bundle_id":"%064d"}\n' 0 \
  >"$WORK/platform-bundle-v0.1.json"
package_args+=(
  --catalog-builder "$CATALOG_BUILDER"
  --main-bin "$MAIN_BIN" --main-source-revision "$MAIN_SOURCE_REVISION"
  --scanout-module "$WORK/mister_magik_scanout_slots.ko"
  --scanout-metadata "$WORK/scanout.metadata.txt"
  --latch-rbf "$WORK/menu-magik-vblank-latch.rbf"
  --latch-metadata "$WORK/latch.metadata.txt"
  --platform-manifest "$WORK/platform-v1.manifest"
  --platform-bundle-manifest "$WORK/platform-bundle-v0.1.json"
)
EXPECT_MAIN=1

ZIP="$("$ROOT/scripts/package-distribution.sh" "${package_args[@]}")"
export ZIP EXPECT_MAIN MAGIK_REVISION MENU_REVISION
python3 - <<'PY'
import os
import sys
import zipfile

zip_path = os.environ["ZIP"]
expect_main = os.environ["EXPECT_MAIN"] == "1"
required = {
    "Scripts/mister-magik.sh",
    "mister-magik/mister-magik-fb",
    "mister-magik/mister-magik-catalog-builder",
    "mister-magik/mame.sqlite3",
    "mister-magik/THIRD-PARTY-NOTICES.txt",
    "mister-magik/SOURCE-OFFER.txt",
    "mister-magik/licenses/MiSTer-MagiK-GPL-3.0-or-later.txt",
    "mister-magik/licenses/RUST-LIBRARIES.txt",
    "mister-magik/licenses/FFMPEG-LGPL-2.1-or-later.txt",
    "mister-magik/licenses/PRESS-START-2P-OFL-1.1.txt",
}
if expect_main:
    required.add("MiSTer_MagiK")
    required.update({
        "mister-magik/platform-v1.manifest",
        "mister-magik/platform-bundle-v0.1.json",
        "mister-magik/game-databases-manifest.json",
        "mister-magik/mister_magik_scanout_slots.ko",
        "mister-magik/mister_magik_scanout_slots.metadata.txt",
        "mister-magik/fpga/menu-magik-vblank-latch.rbf",
        "mister-magik/fpga/menu-magik-vblank-latch.metadata.txt",
    })

with zipfile.ZipFile(zip_path) as zf:
    names = set(zf.namelist())
    release = zf.read("mister-magik/release-v1.txt").decode()
    notices = zf.read("mister-magik/THIRD-PARTY-NOTICES.txt").decode()
    source_offer = zf.read("mister-magik/SOURCE-OFFER.txt").decode()
missing = sorted(required - names)
if missing:
    print(f"package validation failed: missing {', '.join(missing)}", file=sys.stderr)
    sys.exit(1)
legacy_root_paths = {
    "THIRD-PARTY-NOTICES.txt",
    "SOURCE-OFFER.txt",
    "licenses/MiSTer-MagiK-GPL-3.0-or-later.txt",
    "licenses/RUST-LIBRARIES.txt",
    "licenses/FFMPEG-LGPL-2.1-or-later.txt",
    "licenses/PRESS-START-2P-OFL-1.1.txt",
}
unexpected = sorted(legacy_root_paths & names)
if unexpected:
    print(f"package validation failed: legal files outside mister-magik/: {', '.join(unexpected)}", file=sys.stderr)
    sys.exit(1)
if "game_database_version=1" not in release:
    print("package validation failed: missing game database version", file=sys.stderr)
    sys.exit(1)
if "mister_magik_scanout_slots kernel module is also\nGPL-3.0-or-later" not in notices:
    print("package validation failed: missing kernel-module license notice", file=sys.stderr)
    sys.exit(1)
module_source = (
    "https://github.com/NigelBreslaw/MiSTer-MagiK/tree/"
    f"{os.environ['MAGIK_REVISION']}/kernel/scanout-slots"
)
if module_source not in source_offer:
    print("package validation failed: missing exact kernel-module source", file=sys.stderr)
    sys.exit(1)
menu_source = (
    "https://github.com/MiSTer-devel/Menu_MiSTer/tree/"
    f"{os.environ['MENU_REVISION']}"
)
if menu_source not in source_offer:
    print("package validation failed: missing exact Menu_MiSTer source", file=sys.stderr)
    sys.exit(1)

print(f"package validation ok: {zip_path}")
PY

echo
echo "host release gate: ok"
