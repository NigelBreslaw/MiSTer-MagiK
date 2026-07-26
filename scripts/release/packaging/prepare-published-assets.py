#!/usr/bin/env python3
# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

"""Prepare the flat asset layout produced by GitHub Releases."""

from __future__ import annotations

import argparse
import hashlib
import shutil
from pathlib import Path

CHECKSUMS = "SHA256SUMS"


def sha256(path: Path) -> str:
    value = hashlib.sha256()
    with path.open("rb") as source:
        for chunk in iter(lambda: source.read(1024 * 1024), b""):
            value.update(chunk)
    return value.hexdigest()


def prepare(candidate: Path, output: Path) -> None:
    if not candidate.is_dir():
        raise ValueError(f"candidate directory does not exist: {candidate}")
    if output.exists():
        raise ValueError(f"publication output already exists: {output}")

    candidate_resolved = candidate.resolve()
    output_resolved = output.resolve()
    if (
        candidate_resolved == output_resolved
        or candidate_resolved in output_resolved.parents
        or output_resolved in candidate_resolved.parents
    ):
        raise ValueError("candidate and publication output must be separate trees")

    sources: dict[str, Path] = {}
    candidate_checksums = candidate / CHECKSUMS
    for source in sorted(path for path in candidate.rglob("*") if path.is_file()):
        if source == candidate_checksums:
            continue
        name = source.name
        if name == CHECKSUMS or name in sources:
            raise ValueError(f"published release asset collision: {name}")
        sources[name] = source
    if not sources:
        raise ValueError("candidate contains no publishable release assets")

    output.mkdir(parents=True)
    for name, source in sorted(sources.items()):
        shutil.copyfile(source, output / name)
    (output / CHECKSUMS).write_text(
        "".join(f"{sha256(output / name)}  {name}\n" for name in sorted(sources))
    )


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--candidate", required=True, type=Path)
    parser.add_argument("--output", required=True, type=Path)
    args = parser.parse_args()
    prepare(args.candidate, args.output)


if __name__ == "__main__":
    main()
