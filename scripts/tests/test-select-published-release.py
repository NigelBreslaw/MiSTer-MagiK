#!/usr/bin/env python3
# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

from __future__ import annotations

import importlib.util
import json
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

SCRIPT = (
    Path(__file__).resolve().parents[1]
    / "release/databases/select-published-release.py"
)
SPEC = importlib.util.spec_from_file_location("select_published_release", SCRIPT)
assert SPEC and SPEC.loader
selector = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(selector)


def release(tag: str, published: str = "2026-01-01T00:00:00Z", draft: bool = False):
    return {
        "tag_name": tag,
        "published_at": published,
        "draft": draft,
        "prerelease": True,
    }


class PublishedReleaseSelectionTests(unittest.TestCase):
    def test_game_database_versions_are_numeric(self) -> None:
        selected = selector.select_game_databases(
            [
                release("game-databases-v9"),
                release("game-databases-v10", published="2025-01-01T00:00:00Z"),
                release("game-databases-v99", draft=True),
                release("game-databases-v0"),
                release("unrelated-v100"),
            ]
        )
        self.assertEqual(selected["tag_name"], "game-databases-v10")

    def test_game_database_selection_accepts_prereleases(self) -> None:
        self.assertEqual(
            selector.select_game_databases([release("game-databases-v1")])["tag_name"],
            "game-databases-v1",
        )

    def test_missing_game_database_release(self) -> None:
        self.assertIsNone(selector.select_game_databases([release("v0.2.1")]))

    def test_platform_selection_accepts_legacy_hash_tags(self) -> None:
        old = release("platform-v0.1-" + "a" * 64, "2026-01-01T00:00:00Z")
        new = release("platform-v0.1-" + "b" * 64, "2026-02-01T00:00:00Z")
        draft = release("platform-v0.1-" + "c" * 64, "2026-03-01T00:00:00Z", draft=True)
        self.assertEqual(
            selector.select_platform([new, draft, old])["tag_name"], new["tag_name"]
        )

    def test_platform_versions_are_numeric(self) -> None:
        v9 = release("platform-v0.9", "2026-03-01T00:00:00Z")
        v10 = release("platform-v0.10", "2026-01-01T00:00:00Z")
        legacy = release("platform-v0.1-" + "a" * 64, "2026-04-01T00:00:00Z")
        draft = release("platform-v0.99", "2026-05-01T00:00:00Z", draft=True)
        self.assertEqual(
            selector.select_platform([v9, v10, legacy, draft])["tag_name"],
            "platform-v0.10",
        )

    def test_durable_platform_history_is_numeric_and_excludes_legacy_and_drafts(
        self,
    ) -> None:
        releases = [
            release("platform-v0.9", "2026-03-01T00:00:00Z"),
            release("platform-v0.10", "2026-01-01T00:00:00Z"),
            release("platform-v0.1-" + "a" * 64, "2026-04-01T00:00:00Z"),
            release("platform-v0.11", draft=True),
        ]
        self.assertEqual(
            [item["tag_name"] for item in selector.durable_platforms(releases)],
            ["platform-v0.10", "platform-v0.9"],
        )

    def test_platform_version_supports_numbered_and_legacy_tags(self) -> None:
        self.assertEqual(selector.platform_version("platform-v0.12"), 12)
        self.assertEqual(selector.platform_version("platform-v0.1-" + "f" * 64), 1)
        self.assertIsNone(selector.platform_version("platform-v1.2"))

    def test_cli_flattens_paginated_github_payload(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            payload = Path(temporary) / "releases.json"
            payload.write_text(
                json.dumps([[release("v0.2.1")], [release("game-databases-v3")]])
            )
            result = subprocess.run(
                [
                    sys.executable,
                    str(SCRIPT),
                    "game-databases",
                    "--releases",
                    str(payload),
                ],
                check=True,
                capture_output=True,
                text=True,
            )
            self.assertEqual(result.stdout.strip(), "game-databases-v3")

    def test_cli_reports_platform_version(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            payload = Path(temporary) / "releases.json"
            payload.write_text(json.dumps([release("platform-v0.7")]))
            result = subprocess.run(
                [
                    sys.executable,
                    str(SCRIPT),
                    "platform",
                    "--field",
                    "version",
                    "--releases",
                    str(payload),
                ],
                check=True,
                capture_output=True,
                text=True,
            )
            self.assertEqual(result.stdout.strip(), "7")


if __name__ == "__main__":
    unittest.main()
