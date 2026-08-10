#!/usr/bin/env python3
# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

"""Enforce the first-party Slint PixelText8 contract without dependencies."""

from __future__ import annotations

import argparse
from dataclasses import dataclass
from pathlib import Path, PurePosixPath
import re
import subprocess
import sys
from typing import Iterable, NoReturn

UI_ROOT = PurePosixPath("apps/mister/ui")
PRIMITIVE = UI_ROOT / "components/pixel_text_8.slint"
JERSEY_PRIMITIVE = UI_ROOT / "components/jersey_text.slint"
EXPECTED_ENUM_VALUES = ("body12", "px8", "px16", "px24", "px32")

RAW_TEXT = re.compile(r"(?<![A-Za-z0-9_])Text\s*\{")
DIRECT_FONT_SIZE = re.compile(r"(?<![A-Za-z0-9_-])font-size\s*:")
ENUM = re.compile(r"\bexport\s+enum\s+PixelTextSize\s*\{(?P<body>.*?)\}", re.DOTALL)
ENUM_VALUE = re.compile(r"\b(body[0-9]+|px[0-9]+)\s*,")
BODY12_SIZE = re.compile(r"\bsize\s*==\s*PixelTextSize\.body12\s*\?\s*16px\b")
BODY12_FAMILY = re.compile(
    r'\bfont-family\s*:\s*root\.size\s*==\s*PixelTextSize\.body12\s*'
    r'\?\s*"Xerxes 10"\s*:\s*"Press Start 2P"\s*;'
)


@dataclass(frozen=True)
class Source:
    path: PurePosixPath
    text: str


class ContractError(Exception):
    """A deterministic PixelText8 contract violation."""


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("--repository", type=Path, required=True)
    scope = parser.add_mutually_exclusive_group(required=True)
    scope.add_argument("--staged", action="store_true")
    scope.add_argument("--all", action="store_true")
    return parser.parse_args()


def run_git(repository: Path, *args: str) -> bytes:
    try:
        result = subprocess.run(
            ["git", *args],
            cwd=repository,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            check=False,
        )
    except OSError as error:
        raise ContractError(f"cannot run git: {error}") from error
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
            repository,
            "ls-files",
            "--cached",
            "-z",
            "--",
            str(UI_ROOT),
        ).split(b"\0")
        if value
    ]
    sources = []
    for path in paths:
        if path.suffix != ".slint":
            continue
        data = run_git(repository, "show", f":{path}")
        sources.append(Source(path, data.decode("utf-8")))
    return sources


def working_tree_sources(repository: Path) -> list[Source]:
    root = repository / UI_ROOT
    return [
        Source(PurePosixPath(path.relative_to(repository).as_posix()), path.read_text())
        for path in sorted(root.rglob("*.slint"))
    ]


def code_only(text: str) -> str:
    """Replace comments and quoted strings while preserving source line numbers."""

    output: list[str] = []
    index = 0
    state = "code"
    quote = ""
    while index < len(text):
        char = text[index]
        pair = text[index : index + 2]
        if state == "code":
            if pair == "//":
                output.extend("  ")
                index += 2
                state = "line-comment"
            elif pair == "/*":
                output.extend("  ")
                index += 2
                state = "block-comment"
            elif char in {'"', "'"}:
                output.append(" ")
                index += 1
                state = "string"
                quote = char
            else:
                output.append(char)
                index += 1
        elif state == "line-comment":
            output.append("\n" if char == "\n" else " ")
            index += 1
            if char == "\n":
                state = "code"
        elif state == "block-comment":
            if pair == "*/":
                output.extend("  ")
                index += 2
                state = "code"
            else:
                output.append("\n" if char == "\n" else " ")
                index += 1
        else:
            if char == "\\" and index + 1 < len(text):
                output.append(" ")
                output.append("\n" if text[index + 1] == "\n" else " ")
                index += 2
            else:
                output.append("\n" if char == "\n" else " ")
                index += 1
                if char == quote:
                    state = "code"
    return "".join(output)


def line_number(text: str, offset: int) -> int:
    return text.count("\n", 0, offset) + 1


def describe_matches(path: PurePosixPath, code: str, pattern: re.Pattern[str]) -> list[str]:
    return [f"{path}:{line_number(code, match.start())}" for match in pattern.finditer(code)]


def check_primitive(source: Source, code: str) -> list[str]:
    errors = []
    raw_text = describe_matches(source.path, code, RAW_TEXT)
    font_size = describe_matches(source.path, code, DIRECT_FONT_SIZE)
    if len(raw_text) != 1:
        errors.append(f"{source.path}: primitive must contain exactly one raw Text element")
    if len(font_size) != 1:
        errors.append(f"{source.path}: primitive must contain exactly one font-size binding")
    enum_match = ENUM.search(code)
    values = tuple(ENUM_VALUE.findall(enum_match.group("body"))) if enum_match else ()
    if values != EXPECTED_ENUM_VALUES:
        expected = ", ".join(EXPECTED_ENUM_VALUES)
        actual = ", ".join(values) if values else "<missing>"
        errors.append(
            f"{source.path}: PixelTextSize must be exactly [{expected}], got [{actual}]"
        )
    if len(BODY12_SIZE.findall(source.text)) != 1:
        errors.append(f"{source.path}: body12 must resolve to exactly 16px")
    if len(BODY12_FAMILY.findall(source.text)) != 1:
        errors.append(f"{source.path}: body12 must select Xerxes 10 exactly once")
    return errors


def check_jersey_primitive(source: Source, code: str) -> list[str]:
    errors = []
    raw_text = describe_matches(source.path, code, RAW_TEXT)
    font_size = describe_matches(source.path, code, DIRECT_FONT_SIZE)
    if len(raw_text) != 1:
        errors.append(f"{source.path}: Jersey primitive must contain exactly one Text component")
    if len(font_size) != 1:
        errors.append(
            f"{source.path}: Jersey primitive must contain exactly one fixed font-size"
        )
    return errors


def check_sources(sources: Iterable[Source]) -> None:
    violations: list[str] = []
    primitive_seen = False
    jersey_primitive_seen = False
    for source in sources:
        code = code_only(source.text)
        if source.path == PRIMITIVE:
            primitive_seen = True
            violations.extend(check_primitive(source, code))
            continue
        if source.path == JERSEY_PRIMITIVE:
            jersey_primitive_seen = True
            violations.extend(check_jersey_primitive(source, code))
            continue
        for location in describe_matches(source.path, code, RAW_TEXT):
            violations.append(f"{location}: raw Text is forbidden; use PixelText8")
        for location in describe_matches(source.path, code, DIRECT_FONT_SIZE):
            violations.append(
                f"{location}: direct font-size is forbidden; use PixelTextSize"
            )
    if not primitive_seen:
        violations.append(f"{PRIMITIVE}: PixelText8 primitive is missing")
    if not jersey_primitive_seen:
        violations.append(f"{JERSEY_PRIMITIVE}: Jersey text primitive is missing")
    if violations:
        raise ContractError("\n".join(violations))


def fail(message: str) -> NoReturn:
    print(f"error: pixel_text_contract:\n{message}", file=sys.stderr)
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
