#!/usr/bin/env python3
# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

"""Regression checks for the embedded production catalog-builder boundary."""

from pathlib import Path


ROOT = Path(__file__).resolve().parent.parent


def read(path: str) -> str:
    return (ROOT / path).read_text()


def require(text: str, needle: str, context: str) -> None:
    if needle not in text:
        raise AssertionError(f"{context}: missing {needle!r}")


def forbid(text: str, needle: str, context: str) -> None:
    if needle in text:
        raise AssertionError(f"{context}: production still contains {needle!r}")


def main() -> None:
    cargo = read("magik-gui/Cargo.toml")
    require(cargo, 'features = ["builder"]', "frontend catalog dependency")

    worker = read("magik-gui/src/ui_runner/catalog_worker.rs")
    require(worker, "builder_service::run", "catalog worker")
    for needle in (
        "run_catalog_builder_subprocess",
        "MISTER_CATALOG_BUILDER_BIN",
        "Command::new",
    ):
        forbid(worker, needle, "catalog worker")

    main_rs = read("magik-gui/src/main.rs")
    require(main_rs, "builder_service::run", "library-refresh")
    forbid(main_rs, "MISTER_CATALOG_BUILDER_BIN", "library-refresh")

    workflow = read(".github/workflows/distribution.yml")
    forbid(workflow, "scripts/build-catalog-builder.sh", "release workflow")

    package = read("scripts/package-distribution.sh")
    forbid(package, "--catalog-builder", "release package")
    forbid(package, "mister-magik-catalog-builder", "release package")
    require(package, "platform-v2.manifest", "release package")

    manifest = read("scripts/platform-manifest.py")
    require(manifest, 'FORMAT = "mister-magik-platform-v2"', "platform manifest")
    forbid(manifest, '"catalog_builder"', "platform manifest")

    print("embedded catalog release checks ok")


if __name__ == "__main__":
    main()
