#!/usr/bin/env bash
# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
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

scripts/agent ci host-assurance --paths \
  agent-cli/src/main.rs \
  crates/catalog/src/lib.rs \
  crates/magik-core/src/lib.rs \
  crates/framebuffer-stream/src/lib.rs \
  mister/platform/runtime/src/lib.rs \
  agent-cli/src/host/mod.rs \
  mister/tools/agent/src/main.rs \
  apps/mister/src/lib.rs

scripts/agent build release-binaries
apps/mister/scripts/check-arm-shared-libs.sh \
  "$BIN"
apps/mister/scripts/check-arm-shared-libs.sh \
  "$MANAGER"

rm -rf "$WORK"
mkdir -p "$WORK"
python3 - "$WORK/mame.sqlite3" "$WORK/hbmame.sqlite3" <<'PY'
import sqlite3
import sys

mame = sqlite3.connect(sys.argv[1])
mame.executescript("""
CREATE TABLE mame_machines(setname TEXT PRIMARY KEY,parent_setname TEXT,title TEXT NOT NULL,players INTEGER,control_type TEXT,source_version TEXT NOT NULL) WITHOUT ROWID;
WITH RECURSIVE seq(i) AS (VALUES(1) UNION ALL SELECT i+1 FROM seq WHERE i<50000)
INSERT INTO mame_machines SELECT 'machine'||i,'','Machine '||i,1+(i%4),'joy','0.288 (mame0288)' FROM seq;
CREATE TABLE mame_software_items(list_name TEXT NOT NULL,item_name TEXT NOT NULL);
INSERT INTO mame_software_items VALUES('lynx','one'),('megadriv','one'),('n64','one'),('nes','one'),('saturn','one'),('sms','one'),('snes','one');
CREATE TABLE mister_arcade_source(
  id INTEGER PRIMARY KEY CHECK(id=1), schema_version INTEGER NOT NULL,
  repository TEXT NOT NULL, source_path TEXT NOT NULL, source_sha TEXT NOT NULL,
  csv_sha256 TEXT NOT NULL, row_count INTEGER NOT NULL,
  category_count INTEGER NOT NULL
);
CREATE TABLE mister_arcade_entries(
  ordinal INTEGER PRIMARY KEY, raw_json TEXT NOT NULL
);
""")
mame.commit()
mame.close()
hbmame = sqlite3.connect(sys.argv[2])
hbmame.executescript("""
CREATE TABLE mame_machines(setname TEXT PRIMARY KEY,parent_setname TEXT,title TEXT NOT NULL,players INTEGER,control_type TEXT) WITHOUT ROWID;
WITH RECURSIVE seq(i) AS (VALUES(1) UNION ALL SELECT i+1 FROM seq WHERE i<5000)
INSERT INTO mame_machines SELECT 'machine'||i,'','Machine '||i,1+(i%4),'joy' FROM seq;
INSERT INTO mame_machines VALUES('marpy','mappy','Marpy',2,'joy');
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
    "INSERT INTO mister_arcade_entries(ordinal,raw_json) VALUES(?,?)",
    [(ordinal, json.dumps(row)) for ordinal, row in enumerate(rows)],
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
printf 'GPL-3.0-or-later fixture\n' >"$WORK/ArcadeDatabase-LICENSE.txt"

scripts/agent ci game-databases create \
  --mame-sqlite "$WORK/mame.sqlite3" --hbmame-sqlite "$WORK/hbmame.sqlite3" \
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
scripts/agent ci platform-manifest generate \
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

ZIP="$ZIP" python3 - <<'PY'
import hashlib
import os
import sys
import zipfile

required = {
    "Scripts/MiSTer-MagiK.sh",
    "MiSTer_MagiK",
    "mister-magik/mister-magik-fb",
    "mister-magik/mister-magik-manager",
    "mister-magik/mame.sqlite3",
    "mister-magik/hbmame.sqlite3",
    "mister-magik/platform-v3.manifest",
    "mister-magik/platform-bundle-v0.2.json",
    "mister-magik/game-databases-manifest.json",
    "mister-magik/mister_magik_scanout_slots.ko",
    "mister-magik/fpga/menu-magik-vblank-latch.rbf",
    "mister-magik/THIRD-PARTY-NOTICES.txt",
    "mister-magik/SOURCE-OFFER.txt",
}
with zipfile.ZipFile(os.environ["ZIP"]) as archive:
    names = set(archive.namelist())
    manager = archive.read("mister-magik/mister-magik-manager")
    manager_mode = archive.getinfo("mister-magik/mister-magik-manager").external_attr >> 16
    manifest = dict(
        line.split("=", 1)
        for line in archive.read("mister-magik/platform-v3.manifest").decode().splitlines()
        if line and not line.startswith("#")
    )
missing = sorted(required - names)
if missing:
    print(f"package validation failed: missing {', '.join(missing)}", file=sys.stderr)
    raise SystemExit(1)
if manager_mode & 0o111 == 0:
    print("package validation failed: manager is not executable", file=sys.stderr)
    raise SystemExit(1)
if manifest.get("manager_path") != "/media/fat/mister-magik/mister-magik-manager":
    print("package validation failed: manager path is not canonical", file=sys.stderr)
    raise SystemExit(1)
if hashlib.sha256(manager).hexdigest() != manifest.get("manager_sha256"):
    print("package validation failed: manager hash does not match manifest", file=sys.stderr)
    raise SystemExit(1)
forbidden = sorted(
    name for name in names
    if "mister-magik-agent" in name or "mister-magik-dev/" in name or "MiSTer_MagiKDev" in name
)
if forbidden:
    print(f"package validation failed: development payload present: {', '.join(forbidden)}", file=sys.stderr)
    raise SystemExit(1)
print(f"package validation ok: {os.environ['ZIP']}")
PY

echo "host release gate: ok"
