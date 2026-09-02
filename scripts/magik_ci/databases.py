# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

from __future__ import annotations

import csv
import hashlib
import html
import io
import json
import re
import sqlite3
import zipfile
from itertools import pairwise
from pathlib import Path
from typing import Any, cast

from .common import atomic_write, sha256_bytes, sha256_file

FORMAT = "mister-magik-game-databases-manifest-v3"
COMPACT_FORMAT = "mister-magik-game-databases-manifest-v4"
MANIFEST = "game-databases-manifest.json"
CHECKSUMS = "SHA256SUMS"
INDEX = "arcade-updater-index-v1.lz4b"
RUNTIME_METADATA = "magik-metadata-v1.bin"
INPUT_FORMAT = "mister-magik-arcade-updater-inputs-v1"
SOURCE_ORDER = ("distribution", "alternatives", "jtcores", "coinop", "arcade-offset")

# Every MiSTer platform whose catalog resolver consumes MAME software-list
# identities. Keep the original MAME list names here: the runtime collapses
# media-specific lists into the canonical namespace in the second tuple item.
MAME_RUNTIME_SOFTWARE_LISTS: tuple[tuple[str, str, tuple[str, ...]], ...] = (
    ("nes", "nes", ("nes",)),
    ("fds", "fds", ("famicom_flop",)),
    ("snes", "snes", ("snes",)),
    ("n64", "n64", ("n64",)),
    ("sms", "sms", ("sms",)),
    ("megadrive", "megadriv", ("megadriv",)),
    ("s32x", "32x", ("32x",)),
    ("megacd", "megacd", ("megacd",)),
    ("saturn", "saturn", ("saturn",)),
    ("amigacd32", "amigacd32", ("cd32",)),
    ("atarilynx", "lynx", ("lynx",)),
    ("acornatom", "atom", ("atom_cass", "atom_flop", "atom_rom")),
    (
        "acornelectron",
        "electron",
        ("electron_cass", "electron_flop", "electron_rom"),
    ),
    (
        "bbcmicro",
        "bbc",
        (
            "bbc_cass",
            "bbc_flop_32016",
            "bbc_flop_6502",
            "bbc_flop_68000",
            "bbc_flop_80186",
            "bbc_flop_arm",
            "bbc_flop_hybrid",
            "bbc_flop_torch",
            "bbc_flop_z80",
            "bbc_hdd",
            "bbc_rom",
            "bbcb_flop",
            "bbcb_flop_orig",
            "bbcm_cart",
            "bbcm_flop",
        ),
    ),
    (
        "archie",
        "archimedes",
        ("archimedes", "archimedes_hdd", "archimedes_rom"),
    ),
    (
        "apple-ii",
        "apple2",
        (
            "apple2_cass",
            "apple2_flop_clcracked",
            "apple2_flop_misc",
            "apple2_flop_orig",
            "apple2_rom",
        ),
    ),
    (
        "apple-iigs",
        "apple2gs",
        (
            "apple2gs_flop_clcracked",
            "apple2gs_flop_misc",
            "apple2gs_flop_orig",
        ),
    ),
    ("amstrad", "amstrad", ("cpc_cass", "cpc_flop", "gx4000")),
    ("atari2600", "a2600", ("a2600", "a2600_cass")),
    ("atari5200", "a5200", ("a5200",)),
    ("atari7800", "a7800", ("a7800",)),
    ("atari800", "a800", ("a800", "a800_cass", "a800_flop", "xegs")),
    ("atarist", "atarist", ("st_cart", "st_flop", "st_flop_demos")),
    (
        "c64",
        "c64",
        ("c64_cart", "c64_cass", "c64_flop_misc", "c64_flop_orig", "c64_quik"),
    ),
    ("c128", "c128", ("c128_cart", "c128_flop", "c128_rom")),
    ("c16", "c16", ("plus4_cart", "plus4_cass", "plus4_flop", "plus4_quik")),
    ("pet2001", "pet", ("pet_cass", "pet_flop", "pet_hdd", "pet_quik")),
    ("vic20", "vic20", ("vic1001_cart", "vic1001_cass", "vic1001_flop")),
    ("colecovision", "coleco", ("coleco", "coleco_homebrew")),
    ("megaduck", "megaduck", ("megaduck",)),
    ("wonderswan", "wonderswan", ("wswan",)),
    ("wonderswancolor", "wsc", ("wscolor",)),
    ("x68000", "x68000", ("x68k_flop",)),
    (
        "zx-spectrum",
        "spectrum",
        (
            "spectrum_cart",
            "spectrum_cass",
            "spectrum_flop_opus",
            "spectrum_mgt_flop",
            "spectrum_microdrive",
            "spectrum_wafadrive",
        ),
    ),
)

ARCADE_DATABASE_REPOSITORY = "MiSTer-devel/ArcadeDatabase_MiSTer"
ARCADE_DATABASE_PATH = "ArcadeDatabase.csv"
ARCADE_DATABASE_SCHEMA = 1
ARCADE_DATABASE_MAX_SOURCE_BYTES = 16 * 1024 * 1024
ARCADE_DATABASE_MAX_ROWS = 10_000
ARCADE_DATABASE_MAX_FIELD_BYTES = 4 * 1024
ARCADE_DATABASE_REQUIRED_HEADERS = (
    "setname",
    "name",
    "region",
    "version",
    "alternative",
    "parent_title",
    "platform",
    "series",
    "homebrew",
    "bootleg",
    "year",
    "manufacturer",
    "category",
    "linebreak1",
    "resolution",
    "rotation",
    "flip",
    "linebreak2",
    "players",
    "move_inputs",
    "special_controls",
    "num_buttons",
)

_XML_ENTITY_RE = re.compile(r"&(amp|apos|gt|lt|quot|#[0-9]+|#x[0-9a-fA-F]+);")

_MRA_TAG_RE = re.compile(
    r"<\s*(?P<closing>/)?\s*(?P<name>[A-Za-z][A-Za-z0-9_.:-]*)\b(?P<attrs>[^>]*)>",
    re.DOTALL,
)
_MRA_METADATA_FIELDS = (
    "name",
    "rbf",
    "platform",
    "manufacturer",
    "year",
    "setname",
    "parent",
)


def _zip(path: Path, files: list[tuple[str, bytes]]) -> None:
    with zipfile.ZipFile(
        path, "w", compression=zipfile.ZIP_DEFLATED, compresslevel=9
    ) as archive:
        for name, data in files:
            info = zipfile.ZipInfo(name, date_time=(1980, 1, 1, 0, 0, 0))
            info.compress_type = zipfile.ZIP_DEFLATED
            archive.writestr(info, data)


def restore_sources(
    archive: Path, output: Path, source_archive: Path | None = None
) -> None:
    """Restore both SQLite inputs to one layout, independent of release format.

    Release verification belongs to the caller. A present source archive is
    authoritative: corruption must not silently select a different input.
    """
    selected = (
        source_archive
        if source_archive is not None and source_archive.exists()
        else archive
    )
    files: list[tuple[str, bytes]] = []
    try:
        with zipfile.ZipFile(selected) as stream:
            for name in ("mame.sqlite3", "hbmame.sqlite3"):
                if stream.namelist().count(name) != 1:
                    raise ValueError(f"expected exactly one {name}")
                try:
                    data = stream.read(name)
                except (OSError, RuntimeError, zipfile.BadZipFile) as error:
                    raise ValueError(f"cannot read {name}: {error}") from error
                if not data:
                    raise ValueError(f"empty {name}")
                files.append((name, data))
    except (OSError, ValueError, zipfile.BadZipFile) as error:
        raise ValueError(
            f"restore database sources from {selected}: {error}"
        ) from error

    # Validate both members before exposing either input to later build steps.
    for name, data in files:
        atomic_write(output / name, data)


def verify(
    archive: Path,
    manifest: Path | None = None,
    checksums: Path | None = None,
    release_version: int | None = None,
) -> dict[str, object]:
    with zipfile.ZipFile(archive) as stream:
        files = {name: stream.read(name) for name in stream.namelist()}
    if MANIFEST not in files:
        raise ValueError("database_manifest_missing")
    payload = json.loads(files[MANIFEST])
    if payload.get("format") not in {
        FORMAT,
        COMPACT_FORMAT,
        "mister-magik-game-databases-manifest-v2",
        "mister-magik-game-databases-manifest-v1",
    }:
        raise ValueError("invalid_database_manifest")
    if (
        release_version is not None
        and payload.get("release_version") != release_version
    ):
        raise ValueError("database_release_version")
    if manifest is not None and manifest.read_bytes() != files[MANIFEST]:
        raise ValueError("database_manifest_mismatch")
    for line in files.get(CHECKSUMS, b"").decode().splitlines():
        digest, name = line.split("  ", 1)
        if name not in files or sha256_bytes(files[name]) != digest:
            raise ValueError(f"database_checksum:{name}")
    if payload["format"] == COMPACT_FORMAT:
        compact = files.get(RUNTIME_METADATA)
        if compact is None:
            raise ValueError("compact_metadata_missing")
        if b"MMMETA1\0" != compact[:8] or len(compact) < 96:
            raise ValueError("compact_metadata_invalid_header")
        _verify_compact_metadata(compact)
        if "mame.sqlite3" in files or "hbmame.sqlite3" in files:
            raise ValueError("compact_release_contains_sqlite")
    if payload["format"] in {FORMAT, COMPACT_FORMAT}:
        sources = payload.get("sources")
        updater_manifest = (
            sources.get("arcade_updater") if isinstance(sources, dict) else None
        )
        if not isinstance(updater_manifest, dict):
            raise ValueError("invalid_database_manifest: Arcade updater source")
        updater_index = _decode_updater_index_bytes(files[INDEX])
        if set(updater_manifest) == {"builder_sha", "sha256"}:
            # v3 bundles created before updater identity fields were added only
            # recorded the index digest. Keep those immutable releases usable
            # so the next promotion can publish a fully populated manifest.
            if updater_manifest.get("sha256") != sha256_bytes(files[INDEX]):
                raise ValueError("invalid_database_manifest: Arcade updater checksum")
            return payload
        if updater_manifest.get("format") != updater_index["format"]:
            raise ValueError("invalid_database_manifest: Arcade updater format")
        if updater_manifest.get("sources") != updater_index["sources"]:
            raise ValueError("invalid_database_manifest: Arcade updater sources")
        metadata_rows = sum(
            1
            for row in updater_index["rows"]
            if isinstance(row, dict) and row.get("catalog_metadata") is not None
        )
        if updater_manifest.get("catalog_metadata_rows") != metadata_rows:
            raise ValueError(
                "invalid_database_manifest: Arcade updater catalog metadata rows"
            )
    return payload


def _verify_compact_metadata(data: bytes) -> None:
    """Validate the bounded container geometry before it reaches a device."""
    if len(data) > 8 * 1024 * 1024:
        raise ValueError("compact_metadata_size")
    if int.from_bytes(data[8:12], "little") != 1:
        raise ValueError("compact_metadata_version")
    if any(data[12:16]) or any(data[76:96]):
        raise ValueError("compact_metadata_reserved")
    declared = int.from_bytes(data[16:24], "little")
    if declared != len(data):
        raise ValueError("compact_metadata_length")
    shard_count = int.from_bytes(data[24:28], "little")
    index_offset = int.from_bytes(data[28:36], "little")
    entry_size = int.from_bytes(data[36:40], "little")
    index_len = int.from_bytes(data[40:44], "little")
    if index_offset != 96 or entry_size != 128 or index_len != shard_count * entry_size:
        raise ValueError("compact_metadata_index_geometry")
    index_end = index_offset + index_len
    if index_end > len(data):
        raise ValueError("compact_metadata_index_bounds")
    if hashlib.sha256(data[index_offset:index_end]).digest() != data[44:76]:
        raise ValueError("compact_metadata_index_checksum")
    previous = None
    ranges: list[tuple[int, int]] = []
    for offset in range(index_offset, index_end, entry_size):
        entry = data[offset : offset + entry_size]
        raw_id = entry[:32]
        shard_id, separator, padding = raw_id.partition(b"\0")
        if separator and any(padding):
            raise ValueError("compact_metadata_index_id_padding")
        try:
            shard_id.decode("ascii")
        except UnicodeDecodeError as error:
            raise ValueError("compact_metadata_index_id") from error
        if not shard_id or previous is not None and shard_id <= previous:
            raise ValueError("compact_metadata_index_order")
        previous = shard_id
        if entry[32] not in (0, 1):
            raise ValueError("compact_metadata_index_kind")
        if any(entry[33:40]) or any(entry[100:128]):
            raise ValueError("compact_metadata_index_reserved")
        compressed_offset = int.from_bytes(entry[40:48], "little")
        compressed_len = int.from_bytes(entry[48:52], "little")
        decoded_len = int.from_bytes(entry[52:56], "little")
        end = compressed_offset + compressed_len
        if (
            compressed_len == 0
            or compressed_len > 16 * 1024 * 1024
            or decoded_len == 0
            or decoded_len > 16 * 1024 * 1024
        ):
            raise ValueError("compact_metadata_shard_length")
        if compressed_offset < index_end or end > len(data):
            raise ValueError("compact_metadata_shard_bounds")
        ranges.append((compressed_offset, end))
    ranges.sort()
    if any(left[1] > right[0] for left, right in pairwise(ranges)):
        raise ValueError("compact_metadata_shard_overlap")


def extract_release(release: Path, output: Path) -> dict[str, object]:
    manifest = release / MANIFEST
    payload = json.loads(manifest.read_text(encoding="utf-8"))
    archive = release / f"mister-magik-game-databases-v{payload['release_version']}.zip"
    verify(archive, manifest, release / CHECKSUMS, int(payload["release_version"]))
    if any(output.iterdir()) if output.exists() else False:
        raise ValueError("database_extract_not_empty")
    output.mkdir(parents=True, exist_ok=True)
    with zipfile.ZipFile(archive) as stream:
        for name in stream.namelist():
            destination = output / name
            destination.parent.mkdir(parents=True, exist_ok=True)
            destination.write_bytes(stream.read(name))
    return payload


def create(
    *,
    mame: Path,
    hbmame: Path,
    release_version: int,
    mame_tag: str,
    mame_sha: str,
    listxml_asset: str,
    listxml_sha256: str,
    hbmame_tag: str,
    hbmame_sha: str,
    mame_builder_sha: str,
    hbmame_builder_sha: str,
    arcade_database_csv: Path,
    arcade_database_license: Path,
    arcade_database_sha: str,
    arcade_database_builder_sha: str,
    arcade_updater_builder_sha: str,
    arcade_updater_index: Path,
    output: Path,
    runtime_metadata: Path | None = None,
    source_output: Path | None = None,
) -> Path:
    updater_index_payload = _decode_updater_index(arcade_updater_index)
    updater_sources = updater_index_payload["sources"]
    updater_rows = updater_index_payload["rows"]
    runtime_files = [
        ("ArcadeDatabase.csv", arcade_database_csv.read_bytes()),
        ("ArcadeDatabase-LICENSE.txt", arcade_database_license.read_bytes()),
        (INDEX, arcade_updater_index.read_bytes()),
    ]
    if runtime_metadata is None:
        files = [
            ("mame.sqlite3", mame.read_bytes()),
            ("hbmame.sqlite3", hbmame.read_bytes()),
        ] + runtime_files
        format_name = FORMAT
    else:
        files = [(RUNTIME_METADATA, runtime_metadata.read_bytes())] + runtime_files
        format_name = COMPACT_FORMAT
    payload = {
        "format": format_name,
        "release_version": release_version,
        "sources": {
            "mame": {
                "tag": mame_tag,
                "sha": mame_sha,
                "listxml_asset": listxml_asset,
                "listxml_sha256": listxml_sha256,
                "builder_sha": mame_builder_sha,
            },
            "hbmame": {
                "tag": hbmame_tag,
                "sha": hbmame_sha,
                "builder_sha": hbmame_builder_sha,
            },
            "arcade_database": {
                "repository": "MiSTer-devel/ArcadeDatabase_MiSTer",
                "path": "ArcadeDatabase.csv",
                "sha": arcade_database_sha,
                "csv_sha256": sha256_file(arcade_database_csv),
                "license_sha256": sha256_file(arcade_database_license),
                "builder_sha": arcade_database_builder_sha,
            },
            "arcade_updater": {
                "format": updater_index_payload["format"],
                "sha256": sha256_file(arcade_updater_index),
                "sources": updater_sources,
                "catalog_metadata_rows": sum(
                    1
                    for row in updater_rows
                    if isinstance(row, dict) and row.get("catalog_metadata") is not None
                ),
                "builder_sha": arcade_updater_builder_sha,
            },
        },
        "files": [
            {"path": name, "size": len(data), "sha256": sha256_bytes(data)}
            for name, data in files
        ],
    }
    manifest = (json.dumps(payload, indent=2, sort_keys=True) + "\n").encode()
    checksums = (
        "".join(f"{sha256_bytes(data)}  {name}\n" for name, data in files)
        + f"{sha256_bytes(manifest)}  {MANIFEST}\n"
    )
    output.mkdir(parents=True, exist_ok=True)
    archive = output / f"mister-magik-game-databases-v{release_version}.zip"
    _zip(archive, files + [(MANIFEST, manifest), (CHECKSUMS, checksums.encode())])
    atomic_write(output / MANIFEST, manifest)
    atomic_write(output / CHECKSUMS, checksums.encode())
    if source_output is not None:
        source_output.mkdir(parents=True, exist_ok=True)
        source_archive = (
            source_output / f"mister-magik-game-databases-source-v{release_version}.zip"
        )
        _zip(
            source_archive,
            [
                ("mame.sqlite3", mame.read_bytes()),
                ("hbmame.sqlite3", hbmame.read_bytes()),
            ],
        )
    return archive


def update_plan(
    current: dict[str, object] | None,
    *,
    mame_tag: str,
    mame_sha: str,
    hbmame_tag: str,
    hbmame_sha: str,
    arcade_database_sha: str,
    arcade_updater_builder_sha: str,
    revisions: list[str],
) -> dict[str, object]:
    if current is None:
        return {
            "current_version": 0,
            "next_version": 1,
            "mame_changed": True,
            "hbmame_changed": True,
            "arcade_database_changed": True,
            "arcade_updater_changed": True,
            "update_needed": True,
        }
    current_data = cast(dict[str, Any], current)
    sources = cast(dict[str, dict[str, Any]], current_data["sources"])
    mame_changed = (
        sources["mame"]["tag"] != mame_tag or sources["mame"]["sha"] != mame_sha
    )
    hbmame_changed = (
        sources["hbmame"]["tag"] != hbmame_tag or sources["hbmame"]["sha"] != hbmame_sha
    )
    arcade_changed = sources["arcade_database"]["sha"] != arcade_database_sha
    updater_changed = (
        sources["arcade_updater"].get("builder_sha") != arcade_updater_builder_sha
        or sources["arcade_updater"].get("sources") != revisions
    )
    return {
        "current_version": current_data["release_version"],
        "next_version": int(current_data["release_version"]) + 1,
        "mame_changed": mame_changed,
        "hbmame_changed": hbmame_changed,
        "arcade_database_changed": arcade_changed,
        "arcade_updater_changed": updater_changed,
        "update_needed": any(
            (mame_changed, hbmame_changed, arcade_changed, updater_changed)
        ),
    }


def _arcade_decode_text(value: str) -> str:
    if "&" not in value:
        return value
    escaped: list[str] = []
    offset = 0
    while True:
        ampersand = value.find("&", offset)
        if ampersand < 0:
            break
        escaped.append(value[offset:ampersand])
        match = _XML_ENTITY_RE.match(value, ampersand)
        if match is None:
            escaped.append("&amp;")
            offset = ampersand + 1
        else:
            escaped.append(match.group(0))
            offset = match.end()
    escaped.append(value[offset:])
    return html.unescape("".join(escaped))


def _arcade_normalize_key(value: str) -> str:
    normalized: list[str] = []
    last_dash = False
    for character in value.strip().lower():
        if character.isascii() and character.isalnum():
            normalized.append(character)
            last_dash = False
        elif normalized and not last_dash:
            normalized.append("-")
            last_dash = True
    while normalized and normalized[-1] == "-":
        normalized.pop()
    return "".join(normalized)


def _arcade_yes_no(value: str, field: str) -> int:
    normalized = value.strip().lower()
    if normalized == "yes":
        return 1
    if normalized == "no":
        return 0
    raise ValueError(f"ArcadeDatabase {field} must be yes or no; got {value!r}")


def _arcade_source_flag(value: str, field: str) -> int:
    normalized = value.strip().lower()
    if normalized in {"yes", "ys"}:
        return 1
    if normalized in {"", "no"}:
        return 0
    raise ValueError(
        f"ArcadeDatabase {field} must be yes, no, ys, or blank; got {value!r}"
    )


def _arcade_optional_yes_no(value: str, field: str) -> int | None:
    normalized = value.strip().lower()
    if normalized in {"", "n-a"}:
        return None
    if normalized == "yes":
        return 1
    if normalized == "no":
        return 0
    raise ValueError(f"ArcadeDatabase {field} must be yes, no, n-a, or blank")


def _arcade_optional_integer(value: str, field: str) -> int | None:
    normalized = value.strip()
    if not normalized:
        return None
    try:
        return int(normalized)
    except ValueError as error:
        raise ValueError(
            f"invalid ArcadeDatabase {field} {normalized!r}: {error}"
        ) from error


def import_arcade_database(
    *, sqlite: Path, csv_path: Path, source_sha: str
) -> dict[str, object]:
    """Import the pinned ArcadeDatabase CSV into the runtime MAME database."""
    if not re.fullmatch(r"[0-9a-f]{40}", source_sha):
        raise ValueError("source SHA must be 40 lowercase hexadecimal characters")
    source = csv_path.read_bytes()
    if len(source) > ARCADE_DATABASE_MAX_SOURCE_BYTES:
        raise ValueError(
            f"ArcadeDatabase CSV is {len(source)} bytes; "
            f"limit is {ARCADE_DATABASE_MAX_SOURCE_BYTES}"
        )
    records = list(
        csv.reader(io.StringIO(source.decode("utf-8"), newline=""), strict=True)
    )
    if not records:
        raise ValueError("ArcadeDatabase CSV is empty")
    headers = records.pop(0)
    if len(headers) != len(set(headers)):
        raise ValueError("ArcadeDatabase CSV has duplicate headers")
    for required in ARCADE_DATABASE_REQUIRED_HEADERS:
        if required not in headers:
            raise ValueError(f"ArcadeDatabase CSV is missing header {required!r}")
    if len(records) > ARCADE_DATABASE_MAX_ROWS:
        raise ValueError(f"ArcadeDatabase CSV exceeds {ARCADE_DATABASE_MAX_ROWS} rows")

    entries: list[tuple[dict[str, str], dict[str, str]]] = []
    for record in records:
        if len(record) != len(headers):
            raise ValueError(
                f"ArcadeDatabase row has {len(record)} fields; expected {len(headers)}"
            )
        raw = dict(zip(headers, record, strict=True))
        for header, value in raw.items():
            field_bytes = len(value.encode("utf-8"))
            if field_bytes > ARCADE_DATABASE_MAX_FIELD_BYTES:
                raise ValueError(
                    f"ArcadeDatabase field {header!r} is {field_bytes} bytes; "
                    f"limit is {ARCADE_DATABASE_MAX_FIELD_BYTES}"
                )
        entries.append(
            (raw, {name: _arcade_decode_text(value) for name, value in raw.items()})
        )

    categories = len(
        {values["category"] for _, values in entries if values["category"]}
    )
    csv_sha256 = sha256_bytes(source)
    connection = sqlite3.connect(sqlite)
    try:
        machine_table = connection.execute(
            "SELECT count(*) FROM sqlite_master "
            "WHERE type='table' AND name='mame_machines'"
        ).fetchone()[0]
        if machine_table != 1:
            raise ValueError("target SQLite database has no mame_machines table")
        connection.executescript(
            """
            BEGIN IMMEDIATE;
            DROP TABLE IF EXISTS arcade_database;
            DROP TABLE IF EXISTS mister_arcade_entries;
            DROP TABLE IF EXISTS mister_arcade_source;
            CREATE TABLE mister_arcade_source (
                id INTEGER PRIMARY KEY CHECK(id=1),
                schema_version INTEGER NOT NULL,
                repository TEXT NOT NULL,
                source_path TEXT NOT NULL,
                source_sha TEXT NOT NULL,
                csv_sha256 TEXT NOT NULL,
                row_count INTEGER NOT NULL,
                category_count INTEGER NOT NULL
            ) WITHOUT ROWID;
            CREATE TABLE mister_arcade_entries (
                ordinal INTEGER PRIMARY KEY,
                setname TEXT NOT NULL,
                setname_key TEXT NOT NULL,
                name TEXT NOT NULL,
                mra_name_key TEXT NOT NULL,
                region TEXT NOT NULL,
                version TEXT NOT NULL,
                alternative INTEGER NOT NULL,
                parent_title TEXT NOT NULL,
                platform TEXT NOT NULL,
                series TEXT NOT NULL,
                homebrew INTEGER NOT NULL,
                bootleg INTEGER NOT NULL,
                year INTEGER,
                manufacturer TEXT NOT NULL,
                category TEXT NOT NULL,
                resolution TEXT NOT NULL,
                rotation TEXT NOT NULL,
                flip INTEGER,
                players TEXT NOT NULL,
                move_inputs TEXT NOT NULL,
                special_controls TEXT NOT NULL,
                num_buttons INTEGER,
                raw_json TEXT NOT NULL
            );
            CREATE INDEX mister_arcade_entries_setname_idx
                ON mister_arcade_entries(setname_key);
            CREATE INDEX mister_arcade_entries_mra_name_idx
                ON mister_arcade_entries(mra_name_key);
            """
        )
        connection.execute(
            """
            INSERT INTO mister_arcade_source(
                id,schema_version,repository,source_path,source_sha,csv_sha256,
                row_count,category_count
            ) VALUES (1,?,?,?,?,?,?,?)
            """,
            (
                ARCADE_DATABASE_SCHEMA,
                ARCADE_DATABASE_REPOSITORY,
                ARCADE_DATABASE_PATH,
                source_sha,
                csv_sha256,
                len(entries),
                categories,
            ),
        )
        connection.executemany(
            """
            INSERT INTO mister_arcade_entries(
                ordinal,setname,setname_key,name,mra_name_key,region,version,
                alternative,parent_title,platform,series,homebrew,bootleg,year,
                manufacturer,category,resolution,rotation,flip,players,move_inputs,
                special_controls,num_buttons,raw_json
            ) VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)
            """,
            (
                (
                    ordinal,
                    values["setname"],
                    _arcade_normalize_key(values["setname"]),
                    values["name"],
                    f"{values['name'].strip()}.mra".lower(),
                    values["region"],
                    values["version"],
                    _arcade_yes_no(values["alternative"], "alternative"),
                    values["parent_title"],
                    values["platform"],
                    values["series"],
                    _arcade_source_flag(values["homebrew"], "homebrew"),
                    _arcade_source_flag(values["bootleg"], "bootleg"),
                    _arcade_optional_integer(values["year"], "year"),
                    values["manufacturer"],
                    values["category"],
                    values["resolution"],
                    values["rotation"],
                    _arcade_optional_yes_no(values["flip"], "flip"),
                    values["players"],
                    values["move_inputs"],
                    values["special_controls"],
                    _arcade_optional_integer(values["num_buttons"], "num_buttons"),
                    json.dumps(raw, ensure_ascii=False, separators=(",", ":")),
                )
                for ordinal, (raw, values) in enumerate(entries)
            ),
        )
        connection.commit()
    except Exception:
        connection.rollback()
        raise
    finally:
        connection.close()
    return {
        "source_sha": source_sha,
        "csv_sha256": csv_sha256,
        "rows": len(entries),
        "categories": categories,
    }


def build_mame(
    *,
    listxml: Path,
    out: Path,
    software_dir: Path | None = None,
    mame: Path | None = None,
    machine_sqlite: Path | None = None,
) -> None:
    """Build the runtime metadata database from MAME listxml and hash lists.

    ``listxml`` supplies machine metadata. The checked-out MAME ``hash``
    directory supplies software-list identities and ROM/disk hashes. Keep the
    original list names in the release database: the catalog and cloud stager
    canonicalize media-specific aliases (for example ``c64_cart`` and
    ``c64_cass``) when producing one platform asset namespace.
    """
    import xml.etree.ElementTree as ET

    def integer(value: str | None) -> int | None:
        if not value:
            return None
        try:
            return int(value, 0)
        except ValueError:
            try:
                return int(value)
            except ValueError:
                return None

    def decimal(value: str | None) -> float | None:
        if not value:
            return None
        try:
            return float(value)
        except ValueError:
            return None

    def text(element: ET.Element | None, child: str) -> str | None:
        value = element.findtext(child) if element is not None else None
        return value.strip() if value and value.strip() else None

    root = ET.parse(listxml).getroot()
    machine_source_version = root.get("build") or root.get("version") or "mame-listxml"
    out.parent.mkdir(parents=True, exist_ok=True)
    if out.exists():
        out.unlink()
    connection = sqlite3.connect(out)
    connection.executescript(
        """
        CREATE TABLE machines (
            name TEXT PRIMARY KEY,
            description TEXT,
            year TEXT,
            manufacturer TEXT
        );
        CREATE TABLE software (
            shortname TEXT,
            description TEXT,
            year TEXT,
            publisher TEXT
        );
        CREATE TABLE mame_machines (
            setname TEXT PRIMARY KEY,
            parent_setname TEXT,
            title TEXT NOT NULL,
            year TEXT,
            manufacturer TEXT,
            sourcefile TEXT,
            rotate INTEGER,
            display_type TEXT,
            display_width INTEGER,
            display_height INTEGER,
            refresh_hz REAL,
            players INTEGER,
            coins INTEGER,
            control_type TEXT,
            control_ways TEXT,
            buttons INTEGER,
            driver_status TEXT,
            emulation_status TEXT,
            savestate TEXT,
            source_version TEXT NOT NULL
        ) WITHOUT ROWID;
        CREATE TABLE mame_software_items (
            list_name TEXT NOT NULL,
            software_name TEXT NOT NULL,
            parent_name TEXT,
            description TEXT NOT NULL,
            year TEXT,
            publisher TEXT,
            region TEXT,
            source_version TEXT NOT NULL,
            PRIMARY KEY(list_name, software_name)
        ) WITHOUT ROWID;
        CREATE TABLE mame_software_hashes (
            list_name TEXT NOT NULL,
            software_name TEXT NOT NULL,
            part_name TEXT,
            rom_name TEXT,
            size INTEGER,
            crc32 TEXT,
            sha1 TEXT,
            data_area TEXT,
            disk_sha1 TEXT
        );
        CREATE INDEX mame_software_hashes_crc_idx
            ON mame_software_hashes(list_name, size, crc32);
        CREATE INDEX mame_software_hashes_disk_idx
            ON mame_software_hashes(list_name, disk_sha1);
        """
    )
    machine_rows: list[tuple[object, ...]] = []
    for machine in root.findall("machine"):
        name = machine.get("name", "")
        title = text(machine, "description") or name
        year = text(machine, "year")
        manufacturer = text(machine, "manufacturer")
        connection.execute(
            "INSERT OR REPLACE INTO machines VALUES (?, ?, ?, ?)",
            (
                name,
                title,
                year or "",
                manufacturer or "",
            ),
        )
        display = machine.find("display")
        input_node = machine.find("input")
        control = input_node.find("control") if input_node is not None else None
        driver = machine.find("driver")
        machine_rows.append(
            (
                name,
                machine.get("cloneof"),
                title,
                year,
                manufacturer,
                machine.get("sourcefile"),
                integer(display.get("rotate")) if display is not None else None,
                display.get("type") if display is not None else None,
                integer(display.get("width")) if display is not None else None,
                integer(display.get("height")) if display is not None else None,
                decimal(display.get("refresh") if display is not None else None),
                integer(input_node.get("players")) if input_node is not None else None,
                integer(input_node.get("coins")) if input_node is not None else None,
                input_node.get("control") if input_node is not None else None,
                control.get("ways") if control is not None else None,
                integer(control.get("buttons")) if control is not None else None,
                driver.get("status") if driver is not None else None,
                driver.get("emulation") if driver is not None else None,
                driver.get("savestate") if driver is not None else None,
                machine_source_version,
            )
        )
    connection.executemany(
        "INSERT INTO mame_machines VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)",
        machine_rows,
    )

    software_rows: list[tuple[object, ...]] = []
    hash_rows: list[tuple[object, ...]] = []
    if software_dir is not None and software_dir.is_dir():
        for software_path in sorted(software_dir.glob("*.xml")):
            software_root = ET.parse(software_path).getroot()
            list_name = software_root.get("name") or software_path.stem
            software_source_version = (
                software_root.get("build")
                or software_root.get("version")
                or "mame-hash"
            )
            for software in software_root.findall("software"):
                software_name = software.get("name")
                if not software_name:
                    continue
                description = text(software, "description") or software_name
                info = {
                    entry.get("name"): entry.get("value")
                    for entry in software.findall("info")
                    if entry.get("name")
                }
                software_rows.append(
                    (
                        list_name,
                        software_name,
                        software.get("cloneof"),
                        description,
                        text(software, "year"),
                        text(software, "publisher"),
                        info.get("region"),
                        software_source_version,
                    )
                )
                for part in software.findall("part"):
                    part_name = part.get("name")
                    for area in part.findall("dataarea"):
                        data_area = area.get("name")
                        for rom in area.findall("rom"):
                            hash_rows.append(
                                (
                                    list_name,
                                    software_name,
                                    part_name,
                                    rom.get("name"),
                                    integer(rom.get("size")),
                                    rom.get("crc", "").lower() or None,
                                    rom.get("sha1", "").lower() or None,
                                    data_area,
                                    None,
                                )
                            )
                    for disk_area in part.findall("diskarea"):
                        for disk in disk_area.findall("disk"):
                            hash_rows.append(
                                (
                                    list_name,
                                    software_name,
                                    part_name,
                                    disk.get("name"),
                                    None,
                                    None,
                                    None,
                                    disk_area.get("name"),
                                    disk.get("sha1", "").lower() or None,
                                )
                            )
    connection.executemany(
        "INSERT INTO software VALUES (?, ?, ?, ?)",
        ((row[1], row[3], row[4] or "", row[5] or "") for row in software_rows),
    )
    connection.executemany(
        "INSERT INTO mame_software_items VALUES (?,?,?,?,?,?,?,?)",
        software_rows,
    )
    connection.executemany(
        "INSERT INTO mame_software_hashes VALUES (?,?,?,?,?,?,?,?,?)",
        hash_rows,
    )
    connection.commit()
    connection.close()


def mame_runtime_coverage(database: Path) -> dict[str, object]:
    """Report and validate software-list data required by the catalog runtime."""
    connection = sqlite3.connect(f"file:{database}?mode=ro", uri=True)
    try:
        rows_by_list = {
            str(list_name): int(rows)
            for list_name, rows in connection.execute(
                "SELECT list_name, count(*) FROM mame_software_items GROUP BY list_name"
            )
        }
        total_items = int(
            connection.execute("SELECT count(*) FROM mame_software_items").fetchone()[0]
        )
        total_hashes = int(
            connection.execute("SELECT count(*) FROM mame_software_hashes").fetchone()[
                0
            ]
        )
    finally:
        connection.close()

    systems: list[dict[str, object]] = []
    missing: list[str] = []
    for platform_id, canonical_list, source_lists in MAME_RUNTIME_SOFTWARE_LISTS:
        source_rows = {name: rows_by_list.get(name, 0) for name in source_lists}
        system_rows = sum(source_rows.values())
        if system_rows == 0:
            missing.append(platform_id)
        systems.append(
            {
                "platform_id": platform_id,
                "canonical_list": canonical_list,
                "rows": system_rows,
                "source_lists": source_rows,
            }
        )

    report: dict[str, object] = {
        "format": "mister-magik-mame-runtime-coverage-v1",
        "database_list_count": len(rows_by_list),
        "software_item_rows": total_items,
        "software_hash_rows": total_hashes,
        "required_system_count": len(MAME_RUNTIME_SOFTWARE_LISTS),
        "covered_system_count": len(MAME_RUNTIME_SOFTWARE_LISTS) - len(missing),
        "systems": systems,
    }
    if missing:
        raise ValueError("mame_runtime_coverage_missing: " + ", ".join(sorted(missing)))
    return report


def build_updater(input_manifest: Path, output: Path) -> dict[str, object]:
    """Build the Rust-compatible size-prepended LZ4 updater index."""
    import ctypes
    import ctypes.util
    import hashlib

    manifest = json.loads(input_manifest.read_text(encoding="utf-8"))
    sources_manifest = manifest.get("sources")
    source_ids = []
    if isinstance(sources_manifest, list) and all(
        isinstance(source, dict) for source in sources_manifest
    ):
        source_ids = [source.get("id") for source in sources_manifest]
    if manifest.get("format") != INPUT_FORMAT or source_ids != list(SOURCE_ORDER):
        raise ValueError(
            "Arcade updater inputs must contain the five canonical sources "
            "in precedence order"
        )

    sources: list[dict[str, Any]] = []
    rows: dict[str, dict[str, object]] = {}
    source_counts: dict[str, int] = {}
    for source in sources_manifest:
        source_id = source["id"]
        revision = source["revision"]
        _require_lower_hex(f"{source_id} revision", revision, 40)
        database_path = Path(source["database"])
        database_bytes = database_path.read_bytes()
        database_value = json.loads(database_bytes)
        if not isinstance(database_value, dict):
            raise TypeError(f"updater database {database_path} is not an object")
        database = cast(dict[str, Any], database_value)
        sources.append(
            {
                "id": source_id,
                "revision": revision,
                "database_sha256": sha256_bytes(database_bytes),
            }
        )
        files = database.get("files")
        if not isinstance(files, dict):
            raise TypeError(f"updater database {database_path} has no files map")
        count = 0
        for path, entry in files.items():
            normalized = path.lstrip("/").replace("\\", "/")
            if normalized.lower().endswith(".mra") and normalized.startswith(
                "_Arcade/"
            ):
                if not isinstance(entry, dict):
                    raise ValueError(f"updater entry {normalized} is not an object")
                entry_hash = entry["hash"]
                _require_lower_hex("MRA MD5", entry_hash, 32)
                entry_size = entry["size"]
                if (
                    not isinstance(entry_size, int)
                    or isinstance(entry_size, bool)
                    or entry_size < 0
                ):
                    raise ValueError(f"updater size for {normalized} is invalid")
                source_path = _source_path(source, normalized, entry)
                source_bytes = source_path.read_bytes()
                if len(source_bytes) != entry_size:
                    raise ValueError(
                        f"updater size mismatch for {normalized}: "
                        f"database={entry_size} source={len(source_bytes)}"
                    )
                source_hash = hashlib.md5(
                    source_bytes, usedforsecurity=False
                ).hexdigest()
                if source_hash != entry_hash:
                    raise ValueError(f"updater MD5 mismatch for {normalized}")
                header, primary_rom = _mra_inspection(source_bytes, normalized)
                rows[normalized] = {
                    "path": normalized,
                    "source_id": source_id,
                    "size": entry_size,
                    "md5": source_hash,
                    "header": header,
                    "primary_rom": primary_rom,
                }
                count += 1
        source_counts[source_id] = count
    sources.sort(key=lambda value: value["id"])
    ordered_rows = [rows[key] for key in sorted(rows)]
    payload = {"sources": sources, "rows": ordered_rows}
    payload_bytes = json.dumps(
        [sources, ordered_rows], separators=(",", ":"), ensure_ascii=False
    ).encode()
    stored = {
        "format": "mister-magik-arcade-updater-index-v1",
        "payload_sha256": hashlib.sha256(payload_bytes).hexdigest(),
        **payload,
    }
    raw = json.dumps(stored, separators=(",", ":"), ensure_ascii=False).encode()
    library_name = ctypes.util.find_library("lz4")
    if not library_name:
        raise RuntimeError("liblz4 is required to build the updater index")
    library = ctypes.CDLL(library_name)
    compressor = library.LZ4_compress_default
    compressor.argtypes = [ctypes.c_char_p, ctypes.c_char_p, ctypes.c_int, ctypes.c_int]
    compressor.restype = ctypes.c_int
    buffer = ctypes.create_string_buffer(len(raw) + len(raw) // 255 + 16)
    compressed_size = compressor(raw, buffer, len(raw), len(buffer))
    if compressed_size <= 0:
        raise RuntimeError("liblz4 failed to compress updater index")
    encoded = len(raw).to_bytes(4, "little") + buffer.raw[:compressed_size]
    atomic_write(output, encoded)
    return {
        "format": "mister-magik-arcade-updater-index-v1",
        "sources": sources,
        "rows": len(ordered_rows),
        "source_rows": source_counts,
        "catalog_metadata_rows": 0,
        "compressed_bytes": len(encoded),
        "output": str(output),
    }


def _decode_updater_index(path: Path) -> dict[str, Any]:
    """Decode the size-prepended LZ4 index used by the Rust catalog."""
    return _decode_updater_index_bytes(path.read_bytes())


def _decode_updater_index_bytes(encoded: bytes) -> dict[str, Any]:
    import ctypes
    import ctypes.util

    if len(encoded) < 4:
        raise ValueError("Arcade updater index is truncated")
    decoded_size = int.from_bytes(encoded[:4], "little")
    if decoded_size <= 0 or decoded_size > 16 * 1024 * 1024:
        raise ValueError("Arcade updater index decoded size is invalid")
    library_name = ctypes.util.find_library("lz4")
    if not library_name:
        raise RuntimeError("liblz4 is required to read the updater index")
    library = ctypes.CDLL(library_name)
    decompressor = library.LZ4_decompress_safe
    decompressor.argtypes = [
        ctypes.c_void_p,
        ctypes.c_void_p,
        ctypes.c_int,
        ctypes.c_int,
    ]
    decompressor.restype = ctypes.c_int
    source = ctypes.create_string_buffer(encoded[4:])
    destination = ctypes.create_string_buffer(decoded_size)
    result = decompressor(
        source,
        destination,
        len(encoded) - 4,
        decoded_size,
    )
    if result != decoded_size:
        raise ValueError("Arcade updater index decompression failed")
    try:
        payload = json.loads(destination.raw[:result])
    except json.JSONDecodeError as error:
        raise ValueError("Arcade updater index JSON is invalid") from error
    if not isinstance(payload, dict):
        raise TypeError("Arcade updater index payload is not an object")
    if payload.get("format") != "mister-magik-arcade-updater-index-v1":
        raise ValueError("Arcade updater index format is invalid")
    if not isinstance(payload.get("sources"), list) or not isinstance(
        payload.get("rows"), list
    ):
        raise TypeError("Arcade updater index payload is incomplete")
    return cast(dict[str, Any], payload)


def _require_lower_hex(label: str, value: object, length: int) -> None:
    if (
        not isinstance(value, str)
        or len(value) != length
        or any(character not in "0123456789abcdef" for character in value)
    ):
        raise ValueError(f"{label} must be {length} lowercase hexadecimal characters")


def _source_path(
    source: dict[str, Any], normalized: str, entry: dict[str, Any]
) -> Path:
    roots = [Path(value) for value in source.get("roots", [])]
    candidates: list[Path] = []
    if entry.get("arc_at"):
        candidates.extend(root / str(entry["arc_at"]).lstrip("/") for root in roots)
    candidates.extend(root / normalized for root in roots)
    candidates.extend(root / normalized.removeprefix("_Arcade/") for root in roots)
    for candidate in candidates:
        if candidate.is_file():
            return candidate
    raise FileNotFoundError(normalized)


def _mra_inspection(
    data: bytes, path: str
) -> tuple[dict[str, str | None], str | dict[str, object]]:
    text = data.decode("utf-8", errors="replace")
    lower = text.lower()
    if not re.search(r"<\s*misterromdescription(?:\s|>)", lower):
        raise ValueError(f"inspect {path}: missing misterromdescription root")

    tags = list(_MRA_TAG_RE.finditer(text))
    first_rom = next(
        (
            tag
            for tag in tags
            if not tag.group("closing") and tag.group("name").lower() == "rom"
        ),
        None,
    )
    metadata_text = text[: first_rom.start()] if first_rom else text
    header = {
        field: _mra_metadata_value(metadata_text, field)
        for field in _MRA_METADATA_FIELDS
    }

    archive_groups: list[list[tuple[str, str]]] = []
    for tag in tags:
        if tag.group("closing") or tag.group("name").lower() not in {"rom", "part"}:
            continue
        zip_value = _mra_attribute(tag.group("attrs"), "zip")
        if zip_value is None:
            continue
        archives = [
            normalized
            for value in zip_value.split("|")
            if (normalized := _normalize_rom_archive(value)) is not None
        ]
        if archives:
            archive_groups.append(archives)

    archives = sorted({archive for group in archive_groups for archive in group})
    setname = _normalize_rom_setname(header["setname"] or "")
    if not archives:
        primary_rom: str | dict[str, object] = "None"
    elif setname:
        matches = [archive for archive in archives if archive[1] == setname]
        if len(matches) == 1:
            primary_rom = _archive_requirement(matches[0])
        elif not matches and len(archive_groups) == 1:
            primary_rom = _archive_requirement(archive_groups[0][0])
        else:
            primary_rom = "Ambiguous"
    elif len(archives) == 1:
        primary_rom = _archive_requirement(archives[0])
    else:
        primary_rom = "Ambiguous"
    return header, primary_rom


def _mra_metadata_value(text: str, field: str) -> str | None:
    match = re.search(
        rf"<\s*{field}\b[^>]*>(.*?)<\s*/\s*{field}\s*>",
        text,
        flags=re.DOTALL | re.IGNORECASE,
    )
    if match is None:
        return None
    value = match.group(1)
    value = re.sub(r"<!\[CDATA\[(.*?)\]\]>", r"\1", value, flags=re.DOTALL)
    value = html.unescape(value).strip()
    return value or None


def _mra_attribute(attrs: str, name: str) -> str | None:
    match = re.search(
        rf"(?:^|\s){re.escape(name)}\s*=\s*(['\"])(.*?)\1",
        attrs,
        flags=re.DOTALL | re.IGNORECASE,
    )
    if match is None:
        return None
    value = html.unescape(match.group(2)).strip()
    return value or None


def _normalize_rom_archive(value: str) -> tuple[str, str] | None:
    normalized = value.strip().lstrip("/").replace("\\", "/").lower()
    filename = normalized.rsplit("/", 1)[-1]
    if not filename.endswith(".zip"):
        return None
    setname = _normalize_rom_setname(filename)
    if not setname:
        return None
    namespace = "Hbmame" if normalized.startswith("hbmame/") else "Mame"
    return namespace, setname


def _normalize_rom_setname(value: str) -> str:
    return value.strip().removesuffix(".zip").lower()


def _archive_requirement(archive: tuple[str, str]) -> dict[str, object]:
    namespace, setname = archive
    return {"Archive": {"namespace": namespace, "setname": setname}}
