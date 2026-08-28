#!/usr/bin/env python3
# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

"""Regression checks for the embedded production catalog boundary."""

import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]


def read(path: str) -> str:
    return (ROOT / path).read_text()


def require(text: str, needle: str, context: str) -> None:
    if needle not in text:
        raise AssertionError(f"{context}: missing {needle!r}")


def forbid(text: str, needle: str, context: str) -> None:
    if needle in text:
        raise AssertionError(f"{context}: production still contains {needle!r}")


def main() -> None:
    cargo = read("apps/mister/Cargo.toml")
    require(cargo, 'features = ["builder"]', "frontend catalog dependency")

    worker = read("apps/mister/src/ui_runner/catalog_worker.rs")
    require(worker, "execute_planned_fast_refresh", "catalog worker")
    for needle in (
        "run_catalog_builder_subprocess",
        "MISTER_CATALOG_BUILDER_BIN",
        "Command::new",
        "builder_service",
    ):
        forbid(worker, needle, "catalog worker")

    app_entry = read("apps/mister/src/app_entry.rs")
    require(app_entry, "execute_fast_refresh", "library-refresh")
    require(app_entry, "build_fresh_catalog", "library-refresh")
    forbid(app_entry, "builder_service", "library-refresh")
    forbid(app_entry, "MISTER_CATALOG_BUILDER_BIN", "library-refresh")

    workflow = read(".github/workflows/distribution.yml")
    forbid(workflow, "scripts/build-catalog-builder.sh", "release workflow")

    package = read("scripts/package-distribution.sh")
    forbid(package, "--catalog-builder", "release package")
    forbid(package, "mister-magik-catalog-builder", "release package")
    require(package, "PLATFORM_V3_FILE_NAME", "release package")

    subprocess.run(
        [sys.executable, "scripts/checks/generate-platform-v3-consumers.py", "--check"],
        cwd=ROOT,
        check=True,
    )
    constants = read("mister/platform/contracts/generated/platform-v3.constants.sh")
    require(
        constants,
        "PLATFORM_V3_FORMAT='mister-magik-platform-v3'",
        "generated platform manifest constants",
    )
    forbid(constants, "catalog_builder", "generated platform manifest constants")
    for layout in ("public", "development"):
        fixture = read(
            f"mister/platform/contracts/generated/platform-v3.{layout}.fixture"
        )
        require(fixture, "format=mister-magik-platform-v3", f"{layout} fixture")
        forbid(fixture, "catalog_builder", f"{layout} fixture")

    print("embedded catalog release checks ok")


if __name__ == "__main__":
    main()
