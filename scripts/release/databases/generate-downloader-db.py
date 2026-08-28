#!/usr/bin/env python3
# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

"""Generate a MiSTer Downloader v1 database and initial-install bootstrap."""

from __future__ import annotations

import argparse
import hashlib
import json
import re
import zipfile
from pathlib import Path, PurePosixPath

DB_ID = "mister_magik"
DEFAULT_REPOSITORY = "NigelBreslaw/MiSTer-MagiK"
FORBIDDEN_NAMES = {"MiSTer.ini", "menu.rbf", "MiSTer"}
FEED_URL = (
    "https://raw.githubusercontent.com/{repository}/downloader/"
    "mister-magik-{channel}-db.json.zip"
)


def validate_path(value: str) -> None:
    path = PurePosixPath(value)
    if (
        not value
        or value.startswith("/")
        or "//" in value
        or path.is_absolute()
        or ".." in path.parts
        or path.parts[0] in {"linux", "saves"}
        or path.name in FORBIDDEN_NAMES
    ):
        raise ValueError(f"forbidden Downloader path: {value}")
    if path.name.startswith("downloader_") and path.suffix == ".ini":
        raise ValueError(f"Downloader databases may not own drop-ins: {value}")


def write_zip(path: Path, member: str, contents: bytes) -> None:
    info = zipfile.ZipInfo(member, date_time=(1980, 1, 1, 0, 0, 0))
    info.compress_type = zipfile.ZIP_DEFLATED
    info.external_attr = 0o100644 << 16
    with zipfile.ZipFile(path, "w") as archive:
        archive.writestr(info, contents)


def generate(
    receipt_path: Path,
    output: Path,
    channel: str,
    repository: str,
    tag: str,
    timestamp: int,
) -> None:
    receipt = json.loads(receipt_path.read_text())
    version = receipt.get("version", "")
    build = receipt.get("build_number")
    if receipt.get("format") != "mister-magik-release-assets-v1":
        raise ValueError("unsupported release-assets receipt")
    if channel not in {"alpha", "beta", "release"}:
        raise ValueError(f"unsupported release channel: {channel}")
    if channel == "alpha":
        allowed_tags = {"alpha"}
    elif channel == "beta":
        allowed_tags = {"beta", f"v{version}"}
    else:
        allowed_tags = {f"v{version}"}
    if version != f"0.2.{build}" or tag not in allowed_tags:
        raise ValueError("receipt version, build number, and tag disagree")
    if timestamp < 1_000_000_000:
        raise ValueError("timestamp must be a UNIX generation time")
    if not re.fullmatch(r"[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+", repository):
        raise ValueError(f"invalid GitHub repository: {repository}")

    files: dict[str, dict[str, object]] = {}
    folders: set[str] = set()
    for entry in receipt["files"]:
        relative = entry["path"]
        validate_path(relative)
        source = receipt_path.parent / "files" / entry["asset"]
        if not source.is_file() or source.stat().st_size != entry["size"]:
            raise ValueError(
                f"release asset is absent or has wrong size: {entry['asset']}"
            )
        source_bytes = source.read_bytes()
        if hashlib.md5(source_bytes).hexdigest() != entry["md5"]:
            raise ValueError(
                f"release asset MD5 disagrees with receipt: {entry['asset']}"
            )
        if hashlib.sha256(source_bytes).hexdigest() != entry["sha256"]:
            raise ValueError(
                f"release asset SHA-256 disagrees with receipt: {entry['asset']}"
            )
        files[relative] = {
            "hash": entry["md5"],
            "size": entry["size"],
            "url": f"https://github.com/{repository}/releases/download/{tag}/{entry['asset']}",
        }
        parent = PurePosixPath(relative).parent
        while parent != PurePosixPath("."):
            folders.add(parent.as_posix() + "/")
            parent = parent.parent

    database = {
        "v": 1,
        "db_id": DB_ID,
        "timestamp": timestamp,
        "release": {"version": version, "build_number": build},
        "files": files,
        "folders": {folder: {} for folder in sorted(folders)},
    }
    output.mkdir(parents=True, exist_ok=True)
    base = f"mister-magik-{channel}-db.json"
    encoded = (json.dumps(database, indent=2, sort_keys=True) + "\n").encode()
    (output / base).write_bytes(encoded)
    write_zip(output / f"{base}.zip", base, encoded)

    if channel != "alpha":
        ini = (
            "[mister_magik]\n"
            f"db_url = {FEED_URL.format(repository=repository, channel=channel)}\n"
        ).encode()
        write_zip(
            output / f"mister-magik-{channel}-installer.zip",
            "downloader_mister_magik.ini",
            ini,
        )


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--receipt", required=True, type=Path)
    parser.add_argument("--output", required=True, type=Path)
    parser.add_argument(
        "--channel", required=True, choices=("alpha", "beta", "release")
    )
    parser.add_argument("--repository", default=DEFAULT_REPOSITORY)
    parser.add_argument("--tag", required=True)
    parser.add_argument("--timestamp", required=True, type=int)
    args = parser.parse_args()
    generate(
        args.receipt,
        args.output,
        args.channel,
        args.repository,
        args.tag,
        args.timestamp,
    )


if __name__ == "__main__":
    main()
