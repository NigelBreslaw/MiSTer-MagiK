#!/usr/bin/env python3
# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

"""Generate the checked-in 0MHz per-package catalog helper manifest."""

from __future__ import annotations

import argparse
import hashlib
import json
import re
import sqlite3
import sys
import xml.etree.ElementTree as ET
from pathlib import Path, PurePosixPath
from zipfile import ZipFile

SCHEMA = "mister-magik-0mhz-release-manifest-v1"
BARE_AMPERSAND = re.compile(r"&(?!#(?:[0-9]+|x[0-9a-fA-F]+);|[A-Za-z][A-Za-z0-9]+;)")


def arguments() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("--source", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--check", action="store_true")
    return parser.parse_args()


def archive_metadata(source: Path) -> dict[str, dict[str, str]]:
    database = source / "0mhz-dos_meta.sqlite"
    rows: dict[str, dict[str, str]] = {}
    with sqlite3.connect(f"file:{database}?mode=ro", uri=True) as connection:
        for key, headers in connection.execute(
            "SELECT CAST(s3key AS TEXT), CAST(headers AS TEXT) "
            "FROM s3api_per_key_metadata"
        ):
            parsed = {}
            for line in headers.splitlines():
                name, separator, value = line.partition(":")
                if separator:
                    parsed[name.strip().lower()] = value.strip().strip('"')
            rows[key] = parsed
    return rows


def release_identity(source: Path) -> tuple[str, str]:
    metadata = ET.parse(source / "0mhz-dos_meta.xml").getroot()
    identifier = metadata.findtext("identifier") or "0mhz-dos"
    description = metadata.findtext("description") or ""
    match = re.search(r"Collection\s+v([0-9.]+)", description, re.IGNORECASE)
    if not match:
        raise ValueError("0MHz release version is absent from metadata")
    return identifier, match.group(1)


def normalized_member(value: str) -> str:
    return value.replace("\\", "/").lstrip("/")


def parse_launcher(archive: Path, launcher_bytes: bytes) -> ET.Element:
    try:
        text = launcher_bytes.decode("utf-8-sig")
    except UnicodeDecodeError as error:
        raise ValueError(f"{archive.name}: MGL is not UTF-8: {error}") from error
    # The released collection contains valid MiSTer launchers with literal
    # ampersands in path attributes. MiSTer accepts those, while a strict XML
    # parser does not, so escape only bare ampersands before extracting fields.
    compatible_xml = BARE_AMPERSAND.sub("&amp;", text)
    try:
        return ET.fromstring(compatible_xml)
    except ET.ParseError as error:
        raise ValueError(f"{archive.name}: malformed MGL XML: {error}") from error


def package_row(archive: Path, headers: dict[str, str]) -> dict[str, object]:
    with ZipFile(archive) as bundle:
        files = [info for info in bundle.infolist() if not info.is_dir()]
        by_casefold = {normalized_member(info.filename).casefold(): info for info in files}
        launchers = [
            info
            for info in files
            if normalized_member(info.filename).casefold().startswith("_dos games/")
            and normalized_member(info.filename).casefold().endswith(".mgl")
        ]
        if len(launchers) != 1:
            raise ValueError(f"{archive.name}: expected one MGL, found {len(launchers)}")
        launcher = launchers[0]
        launcher_bytes = bundle.read(launcher)
        document = parse_launcher(archive, launcher_bytes)
        rbf = (document.findtext("rbf") or "").strip()
        if not rbf.lower().replace("\\", "/").endswith("ao486"):
            raise ValueError(f"{archive.name}: launcher does not target AO486")
        if not document.findall("reset"):
            raise ValueError(f"{archive.name}: launcher has no reset action")
        payloads = []
        for action in document.findall("file"):
            action_path = normalized_member(action.attrib.get("path", ""))
            expected = normalized_member(f"games/ao486/{action_path}")
            info = by_casefold.get(expected.casefold())
            if info is None:
                raise ValueError(
                    f"{archive.name}: launcher payload is absent from package: {expected}"
                )
            payloads.append(
                {
                    "relative_path": normalized_member(info.filename),
                    "bytes": info.file_size,
                    "crc32": f"{info.CRC:08x}",
                }
            )
        if not payloads:
            raise ValueError(f"{archive.name}: launcher has no payload actions")
        launcher_path = normalized_member(launcher.filename)
        return {
            "package": archive.name,
            "package_bytes": archive.stat().st_size,
            "package_etag": headers.get("etag", ""),
            "title": PurePosixPath(launcher_path).stem,
            "launcher_path": launcher_path,
            "launcher_bytes": launcher.file_size,
            "launcher_crc32": f"{launcher.CRC:08x}",
            "launcher_sha256": hashlib.sha256(launcher_bytes).hexdigest(),
            "payloads": payloads,
        }


def generate(source: Path) -> bytes:
    identifier, version = release_identity(source)
    metadata = archive_metadata(source)
    archives = sorted(source.glob("*.zip"), key=lambda path: path.name.casefold())
    packages = [package_row(archive, metadata.get(archive.name, {})) for archive in archives]
    launchers = [row["launcher_path"].casefold() for row in packages]
    if len(packages) != 319 or len(set(launchers)) != len(packages):
        raise ValueError(
            f"expected 319 unique 0MHz packages, found {len(packages)} / {len(set(launchers))}"
        )
    manifest = {
        "schema": SCHEMA,
        "collection_id": "0mhz",
        "release_id": f"internet-archive-{identifier}-v{version}",
        "source_identifier": identifier,
        "source_version": version,
        "packages": packages,
    }
    return (json.dumps(manifest, indent=2, ensure_ascii=False) + "\n").encode()


def main() -> int:
    args = arguments()
    generated = generate(args.source)
    if args.check:
        current = args.output.read_bytes() if args.output.is_file() else b""
        if current != generated:
            print(f"stale generated manifest: {args.output}", file=sys.stderr)
            return 1
        return 0
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_bytes(generated)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
