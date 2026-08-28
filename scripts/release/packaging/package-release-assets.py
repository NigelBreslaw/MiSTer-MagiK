#!/usr/bin/env python3
# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

"""Flatten an SD-root package into immutable GitHub release assets."""

from __future__ import annotations

import argparse
import hashlib
import json
import shutil
from pathlib import Path


def digest(path: Path, algorithm: str) -> str:
    value = hashlib.new(algorithm)
    with path.open("rb") as source:
        for chunk in iter(lambda: source.read(1024 * 1024), b""):
            value.update(chunk)
    return value.hexdigest()


def github_asset_name(relative: str) -> str:
    return "mister-magik--" + relative.replace("/", "--")


def build_assets(
    stage: Path, archive: Path, output: Path, version: str, build: int
) -> None:
    if version != f"0.2.{build}":
        raise ValueError(f"version/build mismatch: {version} build={build}")
    if not stage.is_dir() or not archive.is_file():
        raise ValueError("stage directory and distribution ZIP must exist")

    files_dir = output / "files"
    if output.exists():
        shutil.rmtree(output)
    files_dir.mkdir(parents=True)

    entries: list[dict[str, object]] = []
    names: set[str] = set()
    for source in sorted(path for path in stage.rglob("*") if path.is_file()):
        relative = source.relative_to(stage).as_posix()
        asset_name = github_asset_name(relative)
        if asset_name in names:
            raise ValueError(f"release asset collision: {asset_name}")
        names.add(asset_name)
        destination = files_dir / asset_name
        shutil.copyfile(source, destination)
        entries.append(
            {
                "path": relative,
                "asset": asset_name,
                "size": source.stat().st_size,
                "md5": digest(source, "md5"),
                "sha256": digest(source, "sha256"),
            }
        )

    archive_copy = output / archive.name
    shutil.copyfile(archive, archive_copy)
    receipt = {
        "format": "mister-magik-release-assets-v1",
        "version": version,
        "build_number": build,
        "archive": archive_copy.name,
        "archive_sha256": digest(archive_copy, "sha256"),
        "files": entries,
    }
    receipt_path = output / "release-assets.json"
    receipt_path.write_text(json.dumps(receipt, indent=2, sort_keys=True) + "\n")

    checksummed = [archive_copy, receipt_path, *sorted(files_dir.iterdir())]
    (output / "SHA256SUMS").write_text(
        "".join(
            f"{digest(path, 'sha256')}  {path.relative_to(output).as_posix()}\n"
            for path in checksummed
        )
    )


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--stage", required=True, type=Path)
    parser.add_argument("--zip", required=True, type=Path)
    parser.add_argument("--output", required=True, type=Path)
    parser.add_argument("--version", required=True)
    parser.add_argument("--build-number", required=True, type=int)
    args = parser.parse_args()
    build_assets(args.stage, args.zip, args.output, args.version, args.build_number)


if __name__ == "__main__":
    main()
