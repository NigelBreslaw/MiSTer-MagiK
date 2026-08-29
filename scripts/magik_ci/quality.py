# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

"""Run the maintained Python quality checks."""

from __future__ import annotations

import subprocess
from pathlib import Path
from typing import Final

QUALITY_COMMANDS: Final[dict[str, tuple[str, ...]]] = {
    "format": (
        "uv",
        "run",
        "ruff",
        "format",
        "--check",
        "scripts",
        "apps/mister/ui_tests",
    ),
    "lint": ("uv", "run", "ruff", "check", "scripts", "apps/mister/ui_tests"),
    "typecheck": ("uv", "run", "ty", "check"),
}


def execute(repository: Path, checks: list[str]) -> None:
    """Run selected checks and report every failure before returning."""
    selected = ["format", "lint", "typecheck"] if checks == ["all"] else checks
    failures: list[str] = []
    for name in selected:
        command = list(QUALITY_COMMANDS[name])
        result = subprocess.run(command, cwd=repository, check=False)
        if result.returncode:
            failures.append(f"{name} (exit {result.returncode})")
    if failures:
        raise RuntimeError("Python quality checks failed: " + ", ".join(failures))
