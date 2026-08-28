#!/usr/bin/env python3
# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

"""Reject maintained references to the retired host package and CLI."""

from __future__ import annotations

import subprocess
import sys
from pathlib import Path

EXCLUDED_PREFIXES = ("history/", "reference/")
FORBIDDEN = (
    "mister/tools/" + "host",
    "mister" + "-tool",
    "scripts/" + "mister",
    "MISTER_" + "TOOL",
    "--mister" + "-tool",
)
RETIRED_COMMANDS = tuple(
    "mister " + command
    for command in (
        "arming-status",
        "catalog",
        "crt",
        "display-matrix",
        "display-mode",
        "media-check",
        "media-download",
        "mode",
        "scene",
        "status",
    )
)


def tracked_files(repository: Path) -> list[Path]:
    result = subprocess.run(
        ["git", "ls-files", "-z"],
        cwd=repository,
        check=True,
        stdout=subprocess.PIPE,
    )
    return [
        repository / raw.decode(errors="surrogateescape")
        for raw in result.stdout.split(b"\0")
        if raw
    ]


def main() -> int:
    repository = Path(sys.argv[1] if len(sys.argv) > 1 else ".").resolve()
    failures: list[str] = []
    for path in tracked_files(repository):
        relative = path.relative_to(repository).as_posix()
        if relative.startswith(EXCLUDED_PREFIXES) or not path.is_file():
            continue
        try:
            text = path.read_text()
        except (OSError, UnicodeError):
            continue
        for spelling in (*FORBIDDEN, *RETIRED_COMMANDS):
            if spelling in text:
                failures.append(f"{relative}: retired spelling {spelling!r}")
    if failures:
        print("unified agent surface check failed:", file=sys.stderr)
        for failure in failures:
            print(f"  {failure}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
