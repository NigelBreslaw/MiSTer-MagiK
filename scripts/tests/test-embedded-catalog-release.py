#!/usr/bin/env python3
# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

"""Regression checks for the embedded production catalog-builder boundary."""

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
    require(worker, "builder_service::run", "catalog worker")
    for needle in (
        "run_catalog_builder_subprocess",
        "MISTER_CATALOG_BUILDER_BIN",
        "Command::new",
    ):
        forbid(worker, needle, "catalog worker")

    main_rs = read("apps/mister/src/main.rs")
    require(main_rs, "builder_service::run", "library-refresh")
    forbid(main_rs, "MISTER_CATALOG_BUILDER_BIN", "library-refresh")

    workflow = read(".github/workflows/distribution.yml")
    forbid(workflow, "scripts/build-catalog-builder.sh", "release workflow")

    package = read("scripts/package-distribution.sh")
    forbid(package, "--catalog-builder", "release package")
    forbid(package, "mister-magik-catalog-builder", "release package")
    require(package, "platform-v3.manifest", "release package")

    manifest = read("agent-cli/src/platform_manifest.rs")
    require(manifest, 'FORMAT: &str = "mister-magik-platform-v3"', "platform manifest")
    forbid(manifest, '"catalog_builder"', "platform manifest")

    print("embedded catalog release checks ok")


if __name__ == "__main__":
    main()
