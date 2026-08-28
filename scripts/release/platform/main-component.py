#!/usr/bin/env python3
# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

"""Verify attested Main_MiSTer component artifacts independently."""

from __future__ import annotations

import argparse
import hashlib
import json
import sys
from pathlib import Path

FORMAT = "mister-magik-main-component-v0.1"
REPOSITORY = "NigelBreslaw/Main_MiSTer"
BRANCH = "mister-magik"
TOOLCHAIN = "gcc-arm-10.2-2020.11-x86_64-arm-none-linux-gnueabihf"
HEX40 = 40
HEX64 = 64


class MainComponentError(ValueError):
    pass


def require_hex(name: str, value: str, length: int) -> None:
    if len(value) != length or any(char not in "0123456789abcdef" for char in value):
        raise MainComponentError(f"invalid {name}")


def digest(path: Path) -> str:
    value = hashlib.sha256()
    with path.open("rb") as source:
        for chunk in iter(lambda: source.read(1024 * 1024), b""):
            value.update(chunk)
    return value.hexdigest()


def component_id(revision: str, toolchain: str = TOOLCHAIN) -> str:
    require_hex("source_revision", revision, HEX40)
    if toolchain != TOOLCHAIN:
        raise MainComponentError("unsupported toolchain")
    material = (
        f"format={FORMAT}\nrepository={REPOSITORY}\nbranch={BRANCH}\n"
        f"source_revision={revision}\ntoolchain={toolchain}\n"
    )
    return hashlib.sha256(material.encode()).hexdigest()


def verify(root: Path, revision: str | None = None) -> dict[str, object]:
    receipt = root / "main-component-v0.1.json"
    binary = root / "MiSTer_MagiK"
    checksums = root / "SHA256SUMS"
    if not receipt.is_file() or not binary.is_file() or not checksums.is_file():
        raise MainComponentError("Main component artifact is incomplete")
    payload = json.loads(receipt.read_text())
    if not isinstance(payload, dict):
        raise MainComponentError("Main component receipt must be an object")
    if (
        payload.get("format") != FORMAT
        or payload.get("repository") != REPOSITORY
        or payload.get("branch") != BRANCH
    ):
        raise MainComponentError("unsupported Main component authority")
    source_revision = payload.get("source_revision")
    toolchain = payload.get("toolchain")
    if not isinstance(source_revision, str) or not isinstance(toolchain, str):
        raise MainComponentError("invalid Main component fields")
    identity = component_id(source_revision, toolchain)
    if payload.get("component_id") != identity:
        raise MainComponentError("Main component identity mismatch")
    if revision is not None and source_revision != revision:
        raise MainComponentError("Main source revision mismatch")
    binary_meta = payload.get("binary")
    if not isinstance(binary_meta, dict) or binary_meta.get("path") != "MiSTer_MagiK":
        raise MainComponentError("invalid Main binary metadata")
    size = binary_meta.get("size")
    sha256 = binary_meta.get("sha256")
    if (
        isinstance(size, bool)
        or not isinstance(size, int)
        or size < 0
        or not isinstance(sha256, str)
    ):
        raise MainComponentError("invalid Main binary identity")
    require_hex("Main binary sha256", sha256, HEX64)
    if size != binary.stat().st_size or sha256 != digest(binary):
        raise MainComponentError("Main binary identity mismatch")
    expected = (
        f"{digest(binary)}  MiSTer_MagiK\n{digest(receipt)}  main-component-v0.1.json\n"
    )
    if checksums.read_text() != expected:
        raise MainComponentError("Main component checksums mismatch")
    return payload


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    identity = commands.add_parser("identity")
    identity.add_argument("--revision", required=True)
    verify_parser = commands.add_parser("verify")
    verify_parser.add_argument("--artifact", type=Path, required=True)
    verify_parser.add_argument("--revision")
    try:
        args = parser.parse_args()
        if args.command == "identity":
            print(component_id(args.revision))
        else:
            print(json.dumps(verify(args.artifact, args.revision), sort_keys=True))
    except (MainComponentError, json.JSONDecodeError, OSError) as error:
        print(f"Main component error: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
