#!/usr/bin/env python3
# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

"""Verify packaged release, channel, and downloader identities agree."""

from __future__ import annotations

import argparse
import json
import re
import zipfile
from pathlib import Path


def require(condition: bool, message: str) -> None:
    if not condition:
        raise ValueError(message)


def verify(
    root: Path,
    version: str,
    build_number: int,
    release_tag: str,
    channel: str,
    candidate_tag: str,
) -> None:
    require(root.is_dir(), f"release asset directory does not exist: {root}")
    require(
        channel in {"alpha", "beta", "release"},
        f"unsupported release channel: {channel}",
    )
    require(version == f"0.2.{build_number}", "version and build number disagree")

    expected_release_tag = {
        "alpha": "alpha",
        "beta": "beta",
        "release": f"v{version}",
    }[channel]
    require(
        release_tag == expected_release_tag,
        f"release tag mismatch: expected {expected_release_tag}, got {release_tag}",
    )
    if channel == "alpha":
        require(
            re.fullmatch(
                rf"alpha-candidate-v{re.escape(version)}-[0-9a-f]{{12}}",
                candidate_tag,
            )
            is not None,
            f"invalid alpha candidate tag: {candidate_tag}",
        )
    else:
        require(
            candidate_tag == release_tag,
            f"candidate tag mismatch: expected {release_tag}, got {candidate_tag}",
        )

    receipt = json.loads((root / "release-assets.json").read_text())
    require(receipt.get("version") == version, "release receipt version mismatch")
    require(
        receipt.get("build_number") == build_number,
        "release receipt build number mismatch",
    )
    archive_name = f"mister-magik-{version}.zip"
    require(receipt.get("archive") == archive_name, "release archive name mismatch")

    with zipfile.ZipFile(root / archive_name) as package:
        release = dict(
            line.split("=", 1)
            for line in package.read("mister-magik/release-v1.txt").decode().splitlines()
        )
        require(
            version.encode() in package.read("mister-magik/mister-magik-fb"),
            "runtime binary does not contain the release version",
        )
        game_databases = json.loads(
            package.read("mister-magik/game-databases-manifest.json")
        )

    require(release.get("version") == version, "packaged release version mismatch")
    require(
        int(release.get("build_number", -1)) == build_number,
        "packaged release build number mismatch",
    )
    require(
        int(release.get("game_database_version", -1))
        == game_databases.get("release_version"),
        "packaged game-database version mismatch",
    )

    database = json.loads((root / f"mister-magik-{channel}-db.json").read_text())
    require(database.get("db_id") == "mister_magik", "downloader database id mismatch")
    require(
        database.get("release")
        == {"version": version, "build_number": build_number},
        "downloader database release identity mismatch",
    )
    require(
        candidate_tag in json.dumps(database),
        "downloader database does not reference the candidate tag",
    )
    if channel == "alpha":
        require(
            not (root / "mister-magik-alpha-installer.zip").exists(),
            "alpha candidates must not publish an installer archive",
        )


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--root", required=True, type=Path)
    parser.add_argument("--version", required=True)
    parser.add_argument("--build-number", required=True, type=int)
    parser.add_argument("--release-tag", required=True)
    parser.add_argument("--channel", required=True)
    parser.add_argument("--candidate-tag", required=True)
    args = parser.parse_args()
    verify(
        args.root,
        args.version,
        args.build_number,
        args.release_tag,
        args.channel,
        args.candidate_tag,
    )


if __name__ == "__main__":
    main()
