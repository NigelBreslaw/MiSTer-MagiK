# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

"""Select the Python tests affected by a pushed path set."""

from __future__ import annotations

import subprocess
from pathlib import Path

SLOW_TEST = "scripts/tests/test-slint-build-contract.py"
PYTHON_CONFIG = {
    ".python-version",
    ".github/actions/setup-python-tools/action.yml",
    ".github/workflows/game-databases.yml",
    ".github/workflows/rust-arm.yml",
    "pyproject.toml",
    "uv.lock",
}


def commands(paths: list[str]) -> list[list[str]]:
    """Return one minimal pytest command for the supplied changed paths."""
    fast = any(
        path.startswith(("scripts/", ".githooks/")) or path in PYTHON_CONFIG
        for path in paths
    )
    slow = any(
        path.startswith(("apps/mister/ui/", "apps/mister/ui-generated/"))
        or path == SLOW_TEST
        for path in paths
    )
    if fast:
        command = ["uv", "run", "pytest", "scripts/tests", "-q"]
        if not slow:
            command.extend(["--ignore", SLOW_TEST])
        return [command]
    if slow:
        return [["python3", SLOW_TEST]]
    return []


def execute(repository: Path, paths: list[str]) -> None:
    """Run affected Python tests, if any."""
    for command in commands(paths):
        subprocess.run(command, cwd=repository, check=True)
