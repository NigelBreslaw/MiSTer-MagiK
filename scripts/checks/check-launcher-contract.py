#!/usr/bin/env python3
# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

"""Ratchet the typed shared launcher contract across Rust and Slint."""

from __future__ import annotations

import argparse
import re
import subprocess
import sys
from collections.abc import Iterable
from dataclasses import dataclass
from pathlib import Path, PurePosixPath
from typing import NoReturn

APP_ROOT = PurePosixPath("apps/mister")
UI_ROOT = APP_ROOT / "ui"
SOURCE_ROOTS = (APP_ROOT / "src", UI_ROOT, APP_ROOT / "examples")
EXCLUDED_UI_PARTS = frozenset({"bench", "experiments", "mockups"})

# Build the retired names so the checker does not flag its own source.
FORBIDDEN_SYMBOLS = (
    "Mister" + "Bridge",
    "Launcher" + "BridgePresenter",
    "Launcher" + "BridgeKey",
)
# Build the retired startup-view spellings so the checker does not flag its own source.
FORBIDDEN_STARTUP_SYMBOLS = (
    "Startup" + "Splash",
    "startup" + "-state",
    "startup" + "_state",
    "startup" + "_splash",
    "Boot" + "Splash",
    "boot" + "_splash",
)
FINITE_NAME_TOKENS = frozenset(
    {
        "activity",
        "availability",
        "choice",
        "focus",
        "hierarchy",
        "kind",
        "mode",
        "orientation",
        "phase",
        "popup",
        "screen",
        "section",
        "state",
        "status",
        "transition",
        "view",
    }
)
LEGACY_DISCRIMINANTS = frozenset(
    {
        "about-selected",
        "confirm-kind",
        "display-combo-open",
        "effective-view",
        "loading-visible",
        "orientation-combo-open",
        "scan-visible",
        "search-pane",
        "search-status",
        "settings-selected",
        "setup-phase",
        "startup-visible",
    }
)
LEGITIMATE_NUMERIC_STATE_NAMES = frozenset({"orientation-confirm-remaining"})

INTEGER_PROPERTY = re.compile(
    r"(?:\b(?:in|out|in-out)\s+)?property\s*<\s*int\s*>\s*"
    r"(?P<name>[A-Za-z][A-Za-z0-9_-]*)"
)
LEGACY_PROPERTY = re.compile(
    r"(?:\b(?:in|out|in-out)\s+)?property\s*<\s*(?:int|string|bool)\s*>\s*"
    r"(?P<name>[A-Za-z][A-Za-z0-9_-]*)"
)
NUMERIC_COMPARISON = re.compile(
    r"(?P<name>[A-Za-z][A-Za-z0-9_.-]*)\s*(?:==|!=)\s*-?\d+"
    r"|-?\d+\s*(?:==|!=)\s*(?P<reverse>[A-Za-z][A-Za-z0-9_.-]*)"
)


@dataclass(frozen=True)
class Source:
    path: PurePosixPath
    text: str


class ContractError(Exception):
    """A deterministic launcher contract violation."""


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("--repository", type=Path, required=True)
    scope = parser.add_mutually_exclusive_group(required=True)
    scope.add_argument("--staged", action="store_true")
    scope.add_argument("--all", action="store_true")
    return parser.parse_args()


def run_git(repository: Path, *args: str) -> bytes:
    result = subprocess.run(
        ["git", *args],
        cwd=repository,
        capture_output=True,
        check=False,
    )
    if result.returncode != 0:
        detail = result.stderr.decode(errors="replace").strip()
        raise ContractError(
            f"git {' '.join(args)} exited {result.returncode}"
            + (f": {detail}" if detail else "")
        )
    return result.stdout


def is_contract_source(path: PurePosixPath) -> bool:
    return path.suffix in {".rs", ".slint"} and any(
        path == root or path.is_relative_to(root) for root in SOURCE_ROOTS
    )


def staged_sources(repository: Path) -> list[Source]:
    paths = [
        PurePosixPath(value.decode(errors="surrogateescape"))
        for value in run_git(
            repository,
            "ls-files",
            "--cached",
            "-z",
            "--",
            *(str(root) for root in SOURCE_ROOTS),
        ).split(b"\0")
        if value
    ]
    return [
        Source(path, run_git(repository, "show", f":{path}").decode("utf-8"))
        for path in paths
        if is_contract_source(path)
    ]


def working_tree_sources(repository: Path) -> list[Source]:
    sources = []
    for root in SOURCE_ROOTS:
        disk_root = repository / root
        if not disk_root.exists():
            continue
        for path in sorted(disk_root.rglob("*")):
            relative = PurePosixPath(path.relative_to(repository).as_posix())
            if path.is_file() and is_contract_source(relative):
                sources.append(Source(relative, path.read_text()))
    return sources


def is_production_slint(source: Source) -> bool:
    return (
        source.path.suffix == ".slint"
        and source.path.is_relative_to(UI_ROOT)
        and EXCLUDED_UI_PARTS.isdisjoint(source.path.parts)
    )


def semantic_tokens(name: str) -> set[str]:
    return set(re.split(r"[-_]", name.rsplit(".", 1)[-1]))


def strip_line_comments(text: str) -> str:
    return re.sub(r"//.*", "", text)


def check_sources(sources: Iterable[Source]) -> None:
    violations: list[str] = []
    for source in sources:
        for symbol in FORBIDDEN_SYMBOLS:
            if symbol in source.text:
                violations.append(
                    f"{source.path}: retired launcher symbol {symbol} is forbidden"
                )
        for symbol in FORBIDDEN_STARTUP_SYMBOLS:
            if symbol in source.text:
                violations.append(
                    f"{source.path}: retired startup-view symbol {symbol} is forbidden"
                )

        if not is_production_slint(source):
            continue
        text = strip_line_comments(source.text)
        for match in INTEGER_PROPERTY.finditer(text):
            name = match.group("name")
            if (
                name not in LEGITIMATE_NUMERIC_STATE_NAMES
                and semantic_tokens(name) & FINITE_NAME_TOKENS
            ):
                violations.append(
                    f"{source.path}: finite launcher property {name} must use an enum, not int"
                )
        for match in LEGACY_PROPERTY.finditer(text):
            name = match.group("name")
            if name in LEGACY_DISCRIMINANTS:
                violations.append(
                    f"{source.path}: legacy launcher discriminant {name} is forbidden"
                )
        for match in NUMERIC_COMPARISON.finditer(text):
            name = match.group("name") or match.group("reverse")
            semantic_name = name.rsplit(".", 1)[-1]
            if (
                semantic_name not in LEGITIMATE_NUMERIC_STATE_NAMES
                and semantic_tokens(name) & FINITE_NAME_TOKENS
            ):
                violations.append(
                    f"{source.path}: typed launcher state {name} cannot be compared with a number"
                )
    if violations:
        raise ContractError("\n".join(violations))


def fail(message: str) -> NoReturn:
    print(f"error: launcher_contract:\n{message}", file=sys.stderr)
    raise SystemExit(1)


def main() -> None:
    args = parse_args()
    repository = args.repository.resolve()
    try:
        sources = (
            staged_sources(repository)
            if args.staged
            else working_tree_sources(repository)
        )
        check_sources(sources)
    except (ContractError, OSError, UnicodeError) as error:
        fail(str(error))


if __name__ == "__main__":
    main()
