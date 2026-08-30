# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

from __future__ import annotations

import html
import json
import re
import sqlite3
import zipfile
from pathlib import Path
from typing import Any, cast

from .common import atomic_write, sha256_bytes, sha256_file

FORMAT = "mister-magik-game-databases-manifest-v3"
MANIFEST = "game-databases-manifest.json"
CHECKSUMS = "SHA256SUMS"
INDEX = "arcade-updater-index-v1.lz4b"
INPUT_FORMAT = "mister-magik-arcade-updater-inputs-v1"
SOURCE_ORDER = ("distribution", "alternatives", "jtcores", "coinop", "arcade-offset")

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
    for line in files.get(CHECKSUMS, b"").decode().splitlines():
        digest, name = line.split("  ", 1)
        if name not in files or sha256_bytes(files[name]) != digest:
            raise ValueError(f"database_checksum:{name}")
    if payload["format"] == FORMAT:
        sources = payload.get("sources")
        updater_manifest = (
            sources.get("arcade_updater")
            if isinstance(sources, dict)
            else None
        )
        if not isinstance(updater_manifest, dict):
            raise ValueError("invalid_database_manifest: Arcade updater source")
        updater_index = _decode_updater_index_bytes(files[INDEX])
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
    updater_index_payload = _decode_updater_index(arcade_updater_index)
    updater_sources = updater_index_payload["sources"]
    updater_rows = updater_index_payload["rows"]
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
        raise ValueError("Arcade updater index payload is not an object")
    if payload.get("format") != "mister-magik-arcade-updater-index-v1":
        raise ValueError("Arcade updater index format is invalid")
    if not isinstance(payload.get("sources"), list) or not isinstance(
        payload.get("rows"), list
    ):
        raise ValueError("Arcade updater index payload is incomplete")
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
