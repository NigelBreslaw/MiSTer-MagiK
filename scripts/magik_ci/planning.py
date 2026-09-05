# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

"""Read-only, bootstrap-free recommendations for a selected change set."""

from __future__ import annotations

import shlex
import subprocess
import tomllib
from pathlib import Path
from typing import TypedDict

from scripts.checks.repository_policy import is_classified

from . import assurance, python_tests, quality


class LocalCheck(TypedDict):
    owner: str
    command: list[str]


class HookChecks(TypedDict):
    pre_commit: str
    pre_push_static: list[list[str]]
    pre_push_quality: list[list[str]]
    pre_push_python_tests: list[list[str]]


class PlanReport(TypedDict):
    schema_version: int
    paths: list[str]
    local_checks: list[LocalCheck]
    hook_checks: HookChecks
    ci_owned_checks: list[str]
    unresolved_coverage: list[str]


CI_BOUNDARY = (
    "pre-push: NOT RUN LOCALLY: Cargo tests/Clippy, ARM builds, visual matrix, "
    "and full Python assurance; a fast-gate pass is not a CI pass"
)


def git_records(repository: Path, *args: str) -> list[str]:
    output = subprocess.check_output(["git", "-C", str(repository), *args])
    return [
        record.decode(errors="surrogateescape")
        for record in output.split(b"\0")
        if record
    ]


def selected_paths(repository: Path, explicit: list[str] | None) -> list[str]:
    paths = (
        explicit
        if explicit is not None
        else [
            *git_records(repository, "diff", "--name-only", "-z"),
            *git_records(repository, "diff", "--cached", "--name-only", "-z"),
            *git_records(
                repository, "ls-files", "--others", "--exclude-standard", "-z"
            ),
        ]
    )
    selected = set()
    for path in paths:
        value = Path(path)
        if ".." in value.parts:
            raise ValueError(f"plan_path_escapes_repository: {path}")
        if value.is_absolute():
            value = value.relative_to(repository)
        if not (repository / value).resolve().is_relative_to(repository):
            raise ValueError(f"plan_path_outside_repository: {path}")
        selected.add(value.as_posix())
    return sorted(selected)


def owning_manifest(repository: Path, path: str) -> Path | None:
    current = repository / path
    if not current.is_dir():
        current = current.parent
    while current.is_relative_to(repository):
        manifest = current / "Cargo.toml"
        if manifest.is_file():
            return manifest
        if current == repository:
            break
        current = current.parent
    return None


def cargo_variants(manifest: str, paths: list[str]) -> list[list[str]]:
    if manifest == "apps/mister/Cargo.toml":
        variants = [["--lib", "--no-default-features"]]
        if any(
            path == "apps/mister/Cargo.toml"
            or path.startswith(
                ("apps/mister/src/ui_", "apps/mister/ui/", "apps/mister/ui-generated/")
            )
            for path in paths
        ):
            variants.append(["--lib", "--no-default-features", "--features", "ui"])
        return variants
    if manifest == "apps/desktop/Cargo.toml":
        return [[], ["--no-default-features", "--features", "compiled-ui"]]
    if manifest == "agent-cli/Cargo.toml" and any(
        "media" in p or p.endswith("Cargo.toml") for p in paths
    ):
        return [[], ["--no-default-features", "--features", "signed-media-manifests"]]
    return [[]]


def report(repository: Path, explicit: list[str] | None = None) -> PlanReport:
    repository = repository.resolve()
    paths = selected_paths(repository, explicit)
    submodules = [
        entry.split("\t", 1)[1]
        for entry in git_records(repository, "ls-files", "--stage", "-z")
        if entry.startswith("160000 ")
    ]
    unresolved: list[str] = [
        f"{path}: unclassified path; extend the repository impact map"
        for path in paths
        if not is_classified(path)
    ]
    manifests: dict[Path, list[str]] = {}
    checks: list[LocalCheck] = []
    python_fixtures: set[str] = set()
    for path in paths:
        if any(path == sub or path.startswith(sub + "/") for sub in submodules):
            unresolved.append(
                f"{path}: independent submodule; validate in its owning repository"
            )
            continue
        value = Path(path)
        if value.suffix == ".py":
            candidates = (
                [value]
                if path.startswith("scripts/tests/")
                else [
                    Path("scripts/tests") / f"test_{value.stem.replace('-', '_')}.py",
                    Path("scripts/tests") / f"test-{value.stem}.py",
                ]
            )
            found = [p.as_posix() for p in candidates if (repository / p).is_file()]
            python_fixtures.update(found)
            if not found:
                unresolved.append(
                    f"{path}: no focused Python fixture mapping; hook/CI selection still applies"
                )
        if value.suffix in {".rs", ".slint"} or value.name in {
            "Cargo.toml",
            "Cargo.lock",
        }:
            manifest = owning_manifest(repository, path)
            if manifest is None:
                unresolved.append(f"{path}: no owning Cargo manifest")
            else:
                manifests.setdefault(manifest, []).append(path)
    for manifest, owned in sorted(manifests.items()):
        relative = manifest.relative_to(repository).as_posix()
        try:
            metadata = tomllib.loads(manifest.read_text())
            package = metadata.get("package", {}).get("name")
        except (OSError, ValueError) as error:
            unresolved.append(f"{relative}: cannot inspect manifest ({error})")
            continue
        if not isinstance(package, str):
            unresolved.append(
                f"{relative}: workspace manifest requires explicit package selection"
            )
            continue
        for flags in cargo_variants(relative, owned):
            selected_features = (
                flags[flags.index("--features") + 1].split(",")
                if "--features" in flags
                else []
            )
            if any(
                feature not in metadata.get("features", {})
                for feature in selected_features
            ):
                unresolved.append(
                    f"{relative}: expected feature combination is unavailable: {selected_features}"
                )
                continue
            for operation in ["test", "clippy"]:
                command = [
                    "scripts/cargo",
                    operation,
                    "--manifest-path",
                    relative,
                    "-p",
                    package,
                    *flags,
                ]
                if operation == "clippy":
                    command.extend(["--", "-D", "warnings"])
                elif "ui" in selected_features:
                    command.extend(["--", "--test-threads=1"])
                checks.append({"owner": package, "command": command})
        if metadata.get("features") and relative not in {
            "apps/mister/Cargo.toml",
            "apps/desktop/Cargo.toml",
            "agent-cli/Cargo.toml",
        }:
            unresolved.append(
                f"{relative}: additional feature combinations need owner review or CI"
            )
    if python_fixtures:
        checks.append(
            {
                "owner": "python",
                "command": ["uv", "run", "pytest", *sorted(python_fixtures), "-q"],
            }
        )
    fast = assurance.fast_checks(repository, paths)
    return {
        "schema_version": 1,
        "paths": paths,
        "local_checks": checks,
        "hook_checks": {
            "pre_commit": "index-only fast gate",
            "pre_push_static": fast,
            "pre_push_quality": [
                list(command) for command in quality.QUALITY_COMMANDS.values()
            ],
            "pre_push_python_tests": python_tests.commands(paths),
        },
        "ci_owned_checks": [
            "full Rust package/feature and reverse-dependency coverage",
            "native Linux assurance",
            "ARM builds",
            "visual matrix",
            "full Python assurance",
        ],
        "unresolved_coverage": sorted(unresolved),
    }


def render(record: PlanReport) -> str:
    hooks = record["hook_checks"]
    assert isinstance(hooks, dict)
    lines = [
        f"pre-push: {len(hooks['pre_push_static'])} fast static checks + {len(hooks['pre_push_quality'])} Python quality checks",
        f"pre-push: {len(hooks['pre_push_python_tests'])} affected Python test command(s)",
        CI_BOUNDARY,
    ]
    checks = record["local_checks"]
    assert isinstance(checks, list)
    lines.extend(
        f"local recommendation: {shlex.join(check['command'])}" for check in checks
    )
    unresolved = record["unresolved_coverage"]
    assert isinstance(unresolved, list)
    lines.extend(f"coverage unresolved: {item}" for item in unresolved)
    return "\n".join(lines)
