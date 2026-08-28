#!/usr/bin/env python3
# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

"""Select current immutable support releases from GitHub Releases JSON."""

from __future__ import annotations

import argparse
import json
import re
import sys
from pathlib import Path

GAME_DATABASE_TAG = re.compile(r"game-databases-v([1-9][0-9]*)")
PLATFORM_TAG = re.compile(r"platform-v0\.([1-9][0-9]*)")
LEGACY_PLATFORM_TAG = re.compile(r"platform-v0\.1-[0-9a-f]{64}")


def select_game_databases(
    releases: list[dict[str, object]],
) -> dict[str, object] | None:
    candidates: list[tuple[int, dict[str, object]]] = []
    for release in releases:
        match = GAME_DATABASE_TAG.fullmatch(str(release.get("tag_name", "")))
        if match and not release.get("draft") and release.get("published_at"):
            candidates.append((int(match.group(1)), release))
    return max(candidates, key=lambda item: item[0])[1] if candidates else None


def platform_version(tag: str) -> int | None:
    match = PLATFORM_TAG.fullmatch(tag)
    if match:
        return int(match.group(1))
    if LEGACY_PLATFORM_TAG.fullmatch(tag):
        return 1
    return None


def select_platform(releases: list[dict[str, object]]) -> dict[str, object] | None:
    candidates: list[tuple[int, str, dict[str, object]]] = []
    for release in releases:
        version = platform_version(str(release.get("tag_name", "")))
        if (
            version is not None
            and not release.get("draft")
            and release.get("published_at")
        ):
            candidates.append((version, str(release["published_at"]), release))
    return (
        max(candidates, key=lambda item: (item[0], item[1]))[2] if candidates else None
    )


def durable_platforms(releases: list[dict[str, object]]) -> list[dict[str, object]]:
    candidates: list[tuple[int, str, dict[str, object]]] = []
    for release in releases:
        match = PLATFORM_TAG.fullmatch(str(release.get("tag_name", "")))
        if match and not release.get("draft") and release.get("published_at"):
            candidates.append(
                (int(match.group(1)), str(release["published_at"]), release)
            )
    return [release for _, _, release in sorted(candidates, reverse=True)]


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("kind", choices=("game-databases", "platform"))
    parser.add_argument("--releases", type=Path)
    parser.add_argument("--field", choices=("tag", "version"), default="tag")
    parser.add_argument("--all", action="store_true")
    args = parser.parse_args()
    try:
        payload = json.loads(
            args.releases.read_text() if args.releases else sys.stdin.read()
        )
        if not isinstance(payload, list):
            raise TypeError("release payload must be an array")
        if payload and all(isinstance(page, list) for page in payload):
            payload = [release for page in payload for release in page]
        if not all(isinstance(release, dict) for release in payload):
            raise TypeError("release payload entries must be objects")
        if args.all:
            if args.kind != "platform" or args.field != "tag":
                raise ValueError("--all is supported only for platform tags")
            for release in durable_platforms(payload):
                print(release["tag_name"])
            return 0
        selected = (
            select_game_databases(payload)
            if args.kind == "game-databases"
            else select_platform(payload)
        )
        if selected is None:
            print(f"no published {args.kind} release found", file=sys.stderr)
            return 1
        tag = str(selected["tag_name"])
        if args.field == "version":
            if args.kind == "game-databases":
                print(GAME_DATABASE_TAG.fullmatch(tag).group(1))
            else:
                print(platform_version(tag))
        else:
            print(tag)
    except (OSError, json.JSONDecodeError, ValueError) as error:
        print(f"release selection failed: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
