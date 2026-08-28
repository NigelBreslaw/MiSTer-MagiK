#!/usr/bin/env python3
# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

"""Enforce the first-party Slint font-specific text component contract."""

from __future__ import annotations

import argparse
import re
import subprocess
import sys
from collections.abc import Iterable
from dataclasses import dataclass
from pathlib import Path, PurePosixPath
from typing import NoReturn

UI_ROOT = PurePosixPath("apps/mister/ui")
RAW_TEXT = re.compile(r"(?<![A-Za-z0-9_])Text\s*\{")
DIRECT_FONT_SIZE = re.compile(r"(?<![A-Za-z0-9_-])font-size\s*:")
ENUM_VALUE = re.compile(r"\b(px[0-9]+)\s*,")
LEGACY_API = re.compile(
    r"\b(?:PixelText8Metrics|PixelText8|PixelTextSize|JerseyTitleText"
    r"|Start2PMetrics|Start2P|Start2PSize)\b"
    r"|pixel_text_8\.slint|jersey_text\.slint|start2p\.slint"
)


@dataclass(frozen=True)
class PrimitiveContract:
    path: PurePosixPath
    component: str
    enum_name: str
    values: tuple[str, ...]
    family: str
    renderer_size: str


CONTRACTS = (
    PrimitiveContract(
        UI_ROOT / "components/yesterday_10.slint",
        "Yesterday10",
        "Yesterday10Size",
        ("px10",),
        "Yesterday 10",
        "16px",
    ),
    PrimitiveContract(
        UI_ROOT / "components/xerxes_10.slint",
        "Xerxes10",
        "Xerxes10Size",
        ("px10",),
        "Xerxes 10",
        "16px",
    ),
    PrimitiveContract(
        UI_ROOT / "components/jersey_25.slint",
        "Jersey25",
        "Jersey25Size",
        ("px25",),
        "Jersey 25",
        "41px",
    ),
    PrimitiveContract(
        UI_ROOT / "components/jersey_15.slint",
        "Jersey15",
        "Jersey15Size",
        ("px15",),
        "Jersey 15",
        "27px",
    ),
    PrimitiveContract(
        UI_ROOT / "components/nocive_15.slint",
        "Nocive15",
        "Nocive15Size",
        ("px15",),
        "Nocive 15",
        "16px",
    ),
    PrimitiveContract(
        UI_ROOT / "components/terminus_8x14.slint",
        "Terminus8x14",
        "Terminus8x14Size",
        ("px8", "px14", "px16", "px24", "px28", "px32"),
        "Terminus 8x14",
        "14px",
    ),
    PrimitiveContract(
        UI_ROOT / "components/spleen_5x8.slint",
        "Spleen5x8",
        "Spleen5x8Size",
        ("px8", "px16"),
        "Spleen 5x8",
        "8px",
    ),
    PrimitiveContract(
        UI_ROOT / "components/spleen_6x12.slint",
        "Spleen6x12",
        "Spleen6x12Size",
        ("px8", "px16", "px24", "px32"),
        "Spleen 6x12",
        "12px",
    ),
)
CONTRACT_BY_PATH = {contract.path: contract for contract in CONTRACTS}


@dataclass(frozen=True)
class Source:
    path: PurePosixPath
    text: str


class ContractError(Exception):
    """A deterministic font text contract violation."""


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


def staged_sources(repository: Path) -> list[Source]:
    paths = [
        PurePosixPath(value.decode(errors="surrogateescape"))
        for value in run_git(
            repository, "ls-files", "--cached", "-z", "--", str(UI_ROOT)
        ).split(b"\0")
        if value
    ]
    return [
        Source(path, run_git(repository, "show", f":{path}").decode("utf-8"))
        for path in paths
        if path.suffix == ".slint"
    ]


def working_tree_sources(repository: Path) -> list[Source]:
    root = repository / UI_ROOT
    return [
        Source(PurePosixPath(path.relative_to(repository).as_posix()), path.read_text())
        for path in sorted(root.rglob("*.slint"))
    ]


def check_primitive(source: Source, contract: PrimitiveContract) -> list[str]:
    errors = []
    if len(RAW_TEXT.findall(source.text)) != 1:
        errors.append(f"{source.path}: {contract.component} must contain one raw Text")
    if len(DIRECT_FONT_SIZE.findall(source.text)) != 1:
        errors.append(
            f"{source.path}: {contract.component} must contain one font-size binding"
        )
    enum = re.search(
        rf"\bexport\s+enum\s+{re.escape(contract.enum_name)}\s*\{{(?P<body>.*?)\}}",
        source.text,
        re.DOTALL,
    )
    values = tuple(ENUM_VALUE.findall(enum.group("body"))) if enum else ()
    if values != contract.values:
        errors.append(
            f"{source.path}: {contract.enum_name} must be exactly {list(contract.values)}, "
            f"got {list(values)}"
        )
    if (
        len(
            re.findall(
                rf'font-family\s*:\s*"{re.escape(contract.family)}"\s*;', source.text
            )
        )
        != 1
    ):
        errors.append(f"{source.path}: family must be {contract.family}")
    if contract.renderer_size not in source.text:
        errors.append(
            f"{source.path}: {contract.component} must resolve to {contract.renderer_size}"
        )
    return errors


def check_sources(sources: Iterable[Source]) -> None:
    violations: list[str] = []
    seen: set[PurePosixPath] = set()
    for source in sources:
        contract = CONTRACT_BY_PATH.get(source.path)
        if contract is not None:
            seen.add(source.path)
            violations.extend(check_primitive(source, contract))
            continue
        if RAW_TEXT.search(source.text):
            violations.append(
                f"{source.path}: raw Text is forbidden; use a font-specific text component"
            )
        if DIRECT_FONT_SIZE.search(source.text):
            violations.append(
                f"{source.path}: direct font-size is forbidden; use the component size enum"
            )
        if LEGACY_API.search(source.text):
            violations.append(f"{source.path}: legacy mixed-font text API is forbidden")
    for contract in CONTRACTS:
        if contract.path not in seen:
            violations.append(
                f"{contract.path}: {contract.component} primitive is missing"
            )
    if violations:
        raise ContractError("\n".join(violations))


def fail(message: str) -> NoReturn:
    print(f"error: font_text_contract:\n{message}", file=sys.stderr)
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
