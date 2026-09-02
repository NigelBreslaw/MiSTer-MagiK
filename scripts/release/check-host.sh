#!/usr/bin/env bash
# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
# shellcheck source=/dev/null
source "$ROOT/mister/platform/contracts/generated/platform-v3.constants.sh"
PUBLIC_ROOT_RELATIVE="${PLATFORM_V3_PUBLIC_ROOT#/media/fat/}"
PUBLIC_MAIN_RELATIVE="${PLATFORM_V3_PUBLIC_MAIN#/media/fat/}"
PUBLIC_GUI_RELATIVE="${PLATFORM_V3_PUBLIC_GUI#/media/fat/}"
PUBLIC_MANAGER_RELATIVE="${PLATFORM_V3_PUBLIC_MANAGER#/media/fat/}"
PUBLIC_SCANOUT_MODULE_RELATIVE="${PLATFORM_V3_PUBLIC_SCANOUT_MODULE#/media/fat/}"
PUBLIC_LATCH_RBF_RELATIVE="${PLATFORM_V3_PUBLIC_LATCH_RBF#/media/fat/}"
DEV_ROOT_RELATIVE="${PLATFORM_V3_DEV_ROOT#/media/fat/}"
BIN="$ROOT/apps/mister/target/armv7-unknown-linux-gnueabihf/release-device/mister-magik-fb"
MANAGER="$ROOT/mister/tools/manager/target/armv7-unknown-linux-gnueabihf/release/mister-magik-manager"
WORK="$ROOT/build/release-check-host"
MAIN_BIN="${MISTER_MAIN_BIN:-$ROOT/../Main_MiSTer/bin/MiSTer}"
cd "$ROOT"

if [[ "${1:-}" == -h || "${1:-}" == --help ]]; then
  echo "usage: scripts/release/check-host.sh"
  exit 0
fi
if [[ $# -gt 0 ]]; then
  echo "ERROR: unknown argument: $1" >&2
  exit 2
fi

scripts/magik-ci ci host-assurance --paths \
  agent-cli/src/main.rs \
  crates/catalog/src/lib.rs \
  crates/magik-core/src/lib.rs \
  crates/framebuffer-stream/src/lib.rs \
  mister/platform/runtime/src/lib.rs \
  agent-cli/src/host/mod.rs \
  mister/tools/agent/src/main.rs \
  apps/mister/src/lib.rs

scripts/magik-ci build release-binaries
scripts/cargo build --locked --manifest-path mister/platform/contracts/manifest/Cargo.toml --bin platform-manifest-check
apps/mister/scripts/check-arm-shared-libs.sh \
  "$BIN"
apps/mister/scripts/check-arm-shared-libs.sh \
  "$MANAGER"

rm -rf "$WORK"
mkdir -p "$WORK"
python3 - "$WORK/mame.sqlite3" "$WORK/hbmame.sqlite3" <<'PY'
import sqlite3
import sys
from scripts.magik_ci.databases import MAME_RUNTIME_SOFTWARE_LISTS

mame = sqlite3.connect(sys.argv[1])
mame.executescript("""
CREATE TABLE mame_machines(setname TEXT PRIMARY KEY,parent_setname TEXT,title TEXT NOT NULL,year TEXT,manufacturer TEXT,players INTEGER,control_type TEXT,source_version TEXT NOT NULL) WITHOUT ROWID;
WITH RECURSIVE seq(i) AS (VALUES(1) UNION ALL SELECT i+1 FROM seq WHERE i<50000)
INSERT INTO mame_machines SELECT 'machine'||i,'','Machine '||i,'1990','Fixture Maker',1+(i%4),'joy','0.288 (mame0288)' FROM seq;
CREATE TABLE mame_software_items(list_name TEXT NOT NULL,software_name TEXT NOT NULL,parent_name TEXT,description TEXT NOT NULL,year TEXT,publisher TEXT,region TEXT,source_version TEXT NOT NULL,PRIMARY KEY(list_name,software_name)) WITHOUT ROWID;
CREATE TABLE mame_software_hashes(list_name TEXT NOT NULL,software_name TEXT NOT NULL,part_name TEXT,rom_name TEXT,size INTEGER,crc32 TEXT,sha1 TEXT,data_area TEXT,disk_sha1 TEXT);
CREATE TABLE mister_arcade_source(
  id INTEGER PRIMARY KEY CHECK(id=1), schema_version INTEGER NOT NULL,
  repository TEXT NOT NULL, source_path TEXT NOT NULL, source_sha TEXT NOT NULL,
  csv_sha256 TEXT NOT NULL, row_count INTEGER NOT NULL,
  category_count INTEGER NOT NULL
);
CREATE TABLE mister_arcade_entries(
  ordinal INTEGER PRIMARY KEY, setname_key TEXT NOT NULL, mra_name_key TEXT NOT NULL,
  name TEXT NOT NULL, category TEXT NOT NULL, year INTEGER, manufacturer TEXT NOT NULL,
  players TEXT NOT NULL, move_inputs TEXT NOT NULL, special_controls TEXT NOT NULL
);
""")
for _, _, source_lists in MAME_RUNTIME_SOFTWARE_LISTS:
    for list_name in source_lists:
        mame.execute(
            "INSERT INTO mame_software_items VALUES(?,?,?,?,?,?,?,?)",
            (list_name, "fixture", None, "Fixture Game", "1990", "Fixture Maker", "USA", "mame0288"),
        )
mame.commit()
mame.close()
hbmame = sqlite3.connect(sys.argv[2])
hbmame.executescript("""
CREATE TABLE mame_machines(setname TEXT PRIMARY KEY,parent_setname TEXT,title TEXT NOT NULL,year TEXT,manufacturer TEXT,players INTEGER,control_type TEXT) WITHOUT ROWID;
WITH RECURSIVE seq(i) AS (VALUES(1) UNION ALL SELECT i+1 FROM seq WHERE i<5000)
INSERT INTO mame_machines SELECT 'machine'||i,'','Machine '||i,'1990','Fixture Maker',1+(i%4),'joy' FROM seq;
INSERT INTO mame_machines VALUES('marpy','mappy','Marpy','1990','Fixture Maker',2,'joy');
CREATE TABLE package_padding(data BLOB NOT NULL);
INSERT INTO package_padding VALUES(zeroblob(1048576));
""")
hbmame.commit()
hbmame.close()
PY
python3 - "$WORK/mame.sqlite3" "$WORK/ArcadeDatabase.csv" <<'PY'
import csv
import hashlib
import json
import sqlite3
import sys

database_path, csv_path = sys.argv[1:]
rows = []
with open(csv_path, "w", newline="", encoding="utf-8") as stream:
    writer = csv.writer(stream)
    writer.writerow(["name", "category"])
    for ordinal in range(2800):
        row = {"name": f"Machine {ordinal}", "category": f"Category {ordinal % 100}"}
        rows.append(row)
        writer.writerow(row.values())
digest = hashlib.sha256(open(csv_path, "rb").read()).hexdigest()
database = sqlite3.connect(database_path)
database.executemany(
    "INSERT INTO mister_arcade_entries(ordinal,setname_key,mra_name_key,name,category,year,manufacturer,players,move_inputs,special_controls) VALUES(?,?,?,?,?,?,?,?,?,?)",
    [
        (
            ordinal,
            f"machine{ordinal}",
            f"fixture-{ordinal}.mra",
            row["name"],
            row["category"],
            1990,
            "Fixture Maker",
            "1",
            "joystick",
            "",
        )
        for ordinal, row in enumerate(rows)
    ],
)
database.execute(
    "INSERT INTO mister_arcade_source VALUES(1,?,?,?,?,?,?,?)",
    (
        1,
        "MiSTer-devel/ArcadeDatabase_MiSTer",
        "ArcadeDatabase.csv",
        "3" * 40,
        digest,
        len(rows),
        100,
    ),
)
database.commit()
PY
cargo run --manifest-path crates/catalog/Cargo.toml --features builder --bin runtime-metadata-builder -- \
  --mame "$WORK/mame.sqlite3" --hbmame "$WORK/hbmame.sqlite3" \
  --output "$WORK/magik-metadata-v1.bin" >/dev/null
printf 'GPL-3.0-or-later fixture\n' >"$WORK/ArcadeDatabase-LICENSE.txt"
mkdir -p "$WORK/updater-source/_Arcade"
printf '<misterromdescription><name>Fixture</name><setname>fixture</setname><rom zip="fixture.zip"/></misterromdescription>\n' \
  >"$WORK/updater-source/_Arcade/Fixture.mra"
python3 - "$WORK" <<'PY'
import hashlib
import json
import pathlib
import sys

work = pathlib.Path(sys.argv[1]).resolve()
mra = work / "updater-source" / "_Arcade" / "Fixture.mra"
data = mra.read_bytes()
sources = []
for position, source_id in enumerate(("distribution", "alternatives", "jtcores", "coinop", "arcade-offset"), 4):
    database = work / f"{source_id}.json"
    database.write_text(json.dumps({"files": {"_Arcade/Fixture.mra": {
        "hash": hashlib.md5(data, usedforsecurity=False).hexdigest(), "size": len(data)
    }}}))
    sources.append({
        "id": source_id,
        "revision": format(position, "x") * 40,
        "database": str(database),
        "roots": [str(work / "updater-source")],
    })
(work / "updater-inputs.json").write_text(json.dumps({
    "format": "mister-magik-arcade-updater-inputs-v1", "sources": sources
}))
PY
scripts/magik-ci ci game-databases build-updater-arcade \
  --input-manifest "$WORK/updater-inputs.json" \
  --out "$WORK/arcade-updater-index-v1.lz4b" >/dev/null

scripts/magik-ci ci game-databases create \
  --mame-sqlite "$WORK/mame.sqlite3" --hbmame-sqlite "$WORK/hbmame.sqlite3" \
  --runtime-metadata "$WORK/magik-metadata-v1.bin" \
  --release-version 1 --mame-tag mame0288 \
  --mame-sha 1111111111111111111111111111111111111111 \
  --mame-listxml-asset mame0288lx.zip \
  --mame-listxml-sha256 "$(printf 5%.0s {1..64})" --hbmame-tag tag24532 \
  --hbmame-sha 2222222222222222222222222222222222222222 \
  --mame-builder-sha "$(git rev-parse HEAD)" \
  --hbmame-builder-sha "$(git rev-parse HEAD)" \
  --arcade-database-csv "$WORK/ArcadeDatabase.csv" \
  --arcade-database-license "$WORK/ArcadeDatabase-LICENSE.txt" \
  --arcade-database-sha 3333333333333333333333333333333333333333 \
  --arcade-database-builder-sha "$(git rev-parse HEAD)" \
  --arcade-updater-index "$WORK/arcade-updater-index-v1.lz4b" \
  --output "$WORK/game-databases" >/dev/null

if [[ -f "$MAIN_BIN" ]]; then
  MAIN_SOURCE_REVISION="$(git -C "$(dirname "$MAIN_BIN")/.." rev-parse HEAD)"
else
  MAIN_BIN="$WORK/MiSTer_MagiK"
  cp "$BIN" "$MAIN_BIN"
  MAIN_SOURCE_REVISION=1111111111111111111111111111111111111111
fi
MAGIK_REVISION="$(git rev-parse HEAD)"
MENU_REVISION=3333333333333333333333333333333333333333
printf 'module release-check\n' >"$WORK/mister_magik_scanout_slots.ko"
printf 'rbf release-check\n' >"$WORK/menu-magik-vblank-latch.rbf"
CONTRACT="$(printf release-check-contract | sha256sum | awk '{print $1}')"
printf 'platform_contract_sha256=%s\nmodule_sha256=%s\nvermagic=5.15.1-MiSTer fixture\nsource_revision=%s\n' \
  "$CONTRACT" "$(sha256sum "$WORK/mister_magik_scanout_slots.ko" | awk '{print $1}')" \
  "$MAGIK_REVISION" >"$WORK/scanout.metadata.txt"
printf 'format=mister-magik-fpga-release-v2\nplatform_contract_sha256=%s\nmagik_commit=%s\nsource_commit=%s\nlatch_protocol_sha256=%064d\nlatch_bridge_sha256=%064d\nlatch_protocol_version=5\nlatch_capability_mask=0x03ff\nrbf_sha256=%s\n' \
  "$CONTRACT" "$MAGIK_REVISION" "$MENU_REVISION" 0 0 \
  "$(sha256sum "$WORK/menu-magik-vblank-latch.rbf" | awk '{print $1}')" \
  >"$WORK/latch.metadata.txt"
printf '{"format":"mister-magik-platform-bundle-v0.2","release_version":16,"bundle_id":"%064d"}\n' 0 \
  >"$WORK/platform-bundle-v0.2.json"
scripts/magik-ci ci platform-manifest generate \
  --layout public --output "$WORK/platform-v3.manifest" \
  --main "$MAIN_BIN" --gui "$BIN" \
  --manager "$MANAGER" \
  --scanout-module "$WORK/mister_magik_scanout_slots.ko" \
  --scanout-metadata "$WORK/scanout.metadata.txt" \
  --latch-rbf "$WORK/menu-magik-vblank-latch.rbf" \
  --latch-metadata "$WORK/latch.metadata.txt" \
  --platform-bundle-manifest "$WORK/platform-bundle-v0.2.json" \
  --main-revision "$MAIN_SOURCE_REVISION" --magik-revision "$MAGIK_REVISION" >/dev/null

VERSION="$(source scripts/lib/bench-context-lib.sh; bench_context_build_receipt_field "$BIN" version)"
BUILD_NUMBER="$(source scripts/lib/bench-context-lib.sh; bench_context_build_receipt_field "$BIN" build_number)"
ZIP="$(scripts/package-distribution.sh \
  --binary "$BIN" \
  --manager "$MANAGER" \
  --game-databases-release-dir "$WORK/game-databases" \
  --name release-check --out-dir "$WORK" \
  --version "$VERSION" --build-number "$BUILD_NUMBER" \
  --release-assets-dir "$WORK/release-assets" \
  --main-bin "$MAIN_BIN" --main-source-revision "$MAIN_SOURCE_REVISION" \
  --scanout-module "$WORK/mister_magik_scanout_slots.ko" \
  --scanout-metadata "$WORK/scanout.metadata.txt" \
  --latch-rbf "$WORK/menu-magik-vblank-latch.rbf" \
  --latch-metadata "$WORK/latch.metadata.txt" \
  --platform-manifest "$WORK/platform-v3.manifest" \
  --platform-bundle-manifest "$WORK/platform-bundle-v0.2.json")"

ZIP="$ZIP" \
PUBLIC_ROOT_RELATIVE="$PUBLIC_ROOT_RELATIVE" \
PUBLIC_MAIN_RELATIVE="$PUBLIC_MAIN_RELATIVE" \
PUBLIC_GUI_RELATIVE="$PUBLIC_GUI_RELATIVE" \
PUBLIC_MANAGER_RELATIVE="$PUBLIC_MANAGER_RELATIVE" \
PUBLIC_SCANOUT_MODULE_RELATIVE="$PUBLIC_SCANOUT_MODULE_RELATIVE" \
PUBLIC_LATCH_RBF_RELATIVE="$PUBLIC_LATCH_RBF_RELATIVE" \
PUBLIC_MANAGER_PATH="$PLATFORM_V3_PUBLIC_MANAGER" \
PLATFORM_V3_FILE_NAME="$PLATFORM_V3_FILE_NAME" \
DEV_ROOT_RELATIVE="$DEV_ROOT_RELATIVE" \
DEV_MAIN_RELATIVE="${PLATFORM_V3_DEV_MAIN#/media/fat/}" \
python3 - <<'PY'
import hashlib
import os
import sys
import zipfile

root = os.environ["PUBLIC_ROOT_RELATIVE"]
main = os.environ["PUBLIC_MAIN_RELATIVE"]
gui = os.environ["PUBLIC_GUI_RELATIVE"]
manager_path = os.environ["PUBLIC_MANAGER_RELATIVE"]
manifest_path = f"{root}/{os.environ['PLATFORM_V3_FILE_NAME']}"
required = {
    "Scripts/MiSTer-MagiK.sh",
    "Scripts/MiSTer-MagiK.platform-v3.constants.sh",
    main,
    gui,
    manager_path,
    f"{root}/magik-metadata-v1.bin",
    f"{root}/arcade-updater-index-v1.lz4b",
    f"{root}/assets/snes/snes-small-v1.rgb565a",
    f"{root}/assets/ui/settings-v1.rgb565a",
    manifest_path,
    f"{root}/platform-bundle-v0.2.json",
    f"{root}/game-databases-manifest.json",
    os.environ["PUBLIC_SCANOUT_MODULE_RELATIVE"],
    os.environ["PUBLIC_LATCH_RBF_RELATIVE"],
    f"{root}/THIRD-PARTY-NOTICES.txt",
    f"{root}/licenses/COMMERCIAL-FONTS.txt",
    f"{root}/SOURCE-OFFER.txt",
}
with zipfile.ZipFile(os.environ["ZIP"]) as archive:
    names = set(archive.namelist())
    manager = archive.read(manager_path)
    snes_artwork = archive.read(f"{root}/assets/snes/snes-small-v1.rgb565a")
    settings_artwork = archive.read(f"{root}/assets/ui/settings-v1.rgb565a")
    manager_mode = archive.getinfo(manager_path).external_attr >> 16
    manifest = dict(
        line.split("=", 1)
        for line in archive.read(manifest_path).decode().splitlines()
        if line and not line.startswith("#")
    )
missing = sorted(required - names)
if missing:
    print(f"package validation failed: missing {', '.join(missing)}", file=sys.stderr)
    raise SystemExit(1)
if manager_mode & 0o111 == 0:
    print("package validation failed: manager is not executable", file=sys.stderr)
    raise SystemExit(1)
if manifest.get("manager_path") != os.environ["PUBLIC_MANAGER_PATH"]:
    print("package validation failed: manager path is not canonical", file=sys.stderr)
    raise SystemExit(1)
if hashlib.sha256(manager).hexdigest() != manifest.get("manager_sha256"):
    print("package validation failed: manager hash does not match manifest", file=sys.stderr)
    raise SystemExit(1)
if hashlib.sha256(snes_artwork).hexdigest() != "7a76993e7e1b0063832b94e9d2ad588549587cf09a14ac2ced72d349ed12f766":
    print("package validation failed: SNES artwork checksum mismatch", file=sys.stderr)
    raise SystemExit(1)
if hashlib.sha256(settings_artwork).hexdigest() != "44d657ff706a49fd8c8999b7c02ea4cdb7e4a8488a54dc68e0b79235dc40e8ec":
    print("package validation failed: settings artwork checksum mismatch", file=sys.stderr)
    raise SystemExit(1)
forbidden = sorted(
    name for name in names
    if "mister-magik-agent" in name
    or f"{os.environ['DEV_ROOT_RELATIVE']}/" in name
    or name == os.environ["DEV_MAIN_RELATIVE"]
    or name.endswith("/mame.sqlite3")
    or name.endswith("/hbmame.sqlite3")
)
if forbidden:
    print(f"package validation failed: development payload present: {', '.join(forbidden)}", file=sys.stderr)
    raise SystemExit(1)
print(f"package validation ok: {os.environ['ZIP']}")
PY

echo "host release gate: ok"
