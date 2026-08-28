from __future__ import annotations

import json
import sqlite3
import zipfile
from pathlib import Path
from typing import Any, cast

from .common import atomic_write, sha256_bytes, sha256_file

FORMAT = "mister-magik-game-databases-manifest-v3"
MANIFEST = "game-databases-manifest.json"
CHECKSUMS = "SHA256SUMS"
INDEX = "arcade-updater-index-v1.lz4b"


def _zip(path: Path, files: list[tuple[str, bytes]]) -> None:
    with zipfile.ZipFile(
        path, "w", compression=zipfile.ZIP_DEFLATED, compresslevel=9
    ) as archive:
        for name, data in files:
            info = zipfile.ZipInfo(name, date_time=(1980, 1, 1, 0, 0, 0))
            info.compress_type = zipfile.ZIP_DEFLATED
            archive.writestr(info, data)


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
    return payload


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
            if name not in {MANIFEST, CHECKSUMS}:
                destination = output / name
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
) -> Path:
    files = [
        ("mame.sqlite3", mame.read_bytes()),
        ("hbmame.sqlite3", hbmame.read_bytes()),
        ("ArcadeDatabase.csv", arcade_database_csv.read_bytes()),
        ("ArcadeDatabase-LICENSE.txt", arcade_database_license.read_bytes()),
        (INDEX, arcade_updater_index.read_bytes()),
    ]
    payload = {
        "format": FORMAT,
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
                "sha256": sha256_file(arcade_updater_index),
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


def build_mame(
    *,
    listxml: Path,
    out: Path,
    software_dir: Path | None = None,
    mame: Path | None = None,
    machine_sqlite: Path | None = None,
) -> None:
    """Build a compact metadata database from MAME listxml.

    The schema is deliberately compatible with the existing host importer;
    source-specific enrichment remains in the release pipeline.
    """
    import xml.etree.ElementTree as ET

    root = ET.parse(listxml).getroot()
    out.parent.mkdir(parents=True, exist_ok=True)
    connection = sqlite3.connect(out)
    connection.executescript(
        "CREATE TABLE IF NOT EXISTS machines (name TEXT PRIMARY KEY, description TEXT, year TEXT, manufacturer TEXT); CREATE TABLE IF NOT EXISTS software (shortname TEXT, description TEXT, year TEXT, publisher TEXT);"
    )
    for machine in root.findall("machine"):
        connection.execute(
            "INSERT OR REPLACE INTO machines VALUES (?, ?, ?, ?)",
            (
                machine.get("name", ""),
                machine.findtext("description", ""),
                machine.findtext("year", ""),
                machine.findtext("manufacturer", ""),
            ),
        )
    connection.commit()
    connection.close()


def build_updater(input_manifest: Path, output: Path) -> dict[str, object]:
    """Build the Rust-compatible size-prepended LZ4 updater index."""
    import ctypes
    import ctypes.util
    import hashlib

    manifest = json.loads(input_manifest.read_text(encoding="utf-8"))
    sources: list[dict[str, Any]] = []
    rows: dict[str, dict[str, object]] = {}
    for source in manifest["sources"]:
        database_bytes = (
            source["database"].read_bytes()
            if isinstance(source["database"], Path)
            else Path(source["database"]).read_bytes()
        )
        database = cast(dict[str, Any], json.loads(database_bytes))
        sources.append(
            {
                "id": source["id"],
                "revision": source["revision"],
                "database_sha256": sha256_bytes(database_bytes),
            }
        )
        for path, entry in database.get("files", {}).items():
            normalized = path.lstrip("/").replace("\\", "/")
            if normalized.lower().endswith(".mra") and normalized.startswith(
                "_Arcade/"
            ):
                rows[normalized] = {
                    "path": normalized,
                    "source_id": source["id"],
                    "size": entry["size"],
                    "md5": entry["hash"],
                    "header": {},
                    "primary_rom": {},
                }
    sources.sort(key=lambda value: value["id"])
    ordered_rows = [rows[key] for key in sorted(rows)]
    payload = {"sources": sources, "rows": ordered_rows}
    payload_bytes = json.dumps(payload, separators=(",", ":"), sort_keys=True).encode()
    stored = {
        "format": "mister-magik-arcade-updater-index-v1",
        "payload_sha256": hashlib.sha256(payload_bytes).hexdigest(),
        **payload,
    }
    raw = json.dumps(stored, separators=(",", ":"), sort_keys=True).encode()
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
        "rows": len(ordered_rows),
        "compressed_bytes": len(encoded),
        "output": str(output),
    }
