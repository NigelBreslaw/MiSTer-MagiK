# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

"""Fast, bootstrap-free repository assurance checks."""

from __future__ import annotations

import subprocess
from pathlib import Path, PurePosixPath


def _run(repository: Path, command: list[str]) -> None:
    subprocess.run(command, cwd=repository, check=True)


def _is_shell(path: str) -> bool:
    value = PurePosixPath(path)
    return value.suffix == ".sh" or value.name in {"agent", "mister"}


def fast_checks(repository: Path, paths: list[str]) -> list[list[str]]:
    """Return the fast checks selected by the supplied repository paths."""
    checks = [
        ["scripts/checks/check-repository-layout.py"],
        ["scripts/checks/check-unified-agent-surface.py"],
    ]
    if any(
        path.startswith(("scripts", "docs"))
        or (path.startswith("apps/mister/ui/") and path.endswith(".slint"))
        for path in paths
    ):
        checks.append(
            [
                "scripts/checks/check-font-text-contract.py",
                "--repository",
                ".",
                "--all",
            ]
        )
    if any(
        path.startswith(("apps/mister/src", "apps/mister/ui", "apps/mister/examples"))
        or path
        in {
            "scripts/checks/check-launcher-contract.py",
            "scripts/tests/test-launcher-contract.py",
        }
        for path in paths
    ):
        checks.append(
            [
                "scripts/checks/check-launcher-contract.py",
                "--repository",
                ".",
                "--all",
            ]
        )
    checks.extend(
        ["bash", "-n", path]
        for path in paths
        if _is_shell(path) and (repository / path).exists()
    )
    return checks


def execute(repository: Path, paths: list[str]) -> None:
    """Run selected fast checks; failures are reported by their subprocess."""
    for command in fast_checks(repository, paths):
        _run(repository, command)
