# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

"""Shared repository path policy for local hooks."""

from __future__ import annotations

from pathlib import PurePosixPath

CLASSIFIED_PREFIXES = (
    ".github",
    ".githooks",
    "LICENSES",
    "agent-cli",
    "apps/desktop",
    "apps/framebuffer-lab",
    "apps/framebuffer-scene-lab",
    "apps/mister",
    "crates",
    "docs",
    "documentation",
    "history",
    "mister/platform/contracts",
    "mister/platform/fpga",
    "mister/platform/kernel",
    "mister/platform/runtime",
    "mister/tools/agent",
    "mister/tools/manager",
    "private",
    "scripts",
    "tools",
)


def is_classified(path: str) -> bool:
    value = PurePosixPath(path)
    first = value.parts[0] if value.parts else ""
    if path in {"Cargo.toml", "Cargo.lock"}:
        return True
    if len(value.parts) == 1 or value.name == "AGENTS.md":
        return True
    if first.startswith(".") and first not in {".github", ".githooks"}:
        return True
    return any(
        path == prefix or path.startswith(f"{prefix}/")
        for prefix in CLASSIFIED_PREFIXES
    )
