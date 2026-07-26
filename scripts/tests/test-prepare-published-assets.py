#!/usr/bin/env python3
# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

from __future__ import annotations

import hashlib
import importlib.util
import tempfile
import unittest
from pathlib import Path

SCRIPT = (
    Path(__file__).resolve().parents[1]
    / "release/packaging/prepare-published-assets.py"
)
SPEC = importlib.util.spec_from_file_location("prepare_published_assets", SCRIPT)
assert SPEC and SPEC.loader
module = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(module)


class PublishedReleaseAssetTests(unittest.TestCase):
    def test_checksums_match_flat_downloaded_layout(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            candidate = root / "candidate"
            (candidate / "files").mkdir(parents=True)
            (candidate / "files/mister-magik--MiSTer_MagiK").write_bytes(b"main")
            (candidate / "release-assets.json").write_bytes(b"receipt")
            (candidate / "SHA256SUMS").write_text(
                "stale  files/mister-magik--MiSTer_MagiK\n"
            )
            output = root / "published"

            module.prepare(candidate, output)

            self.assertEqual(
                sorted(path.name for path in output.iterdir()),
                ["SHA256SUMS", "mister-magik--MiSTer_MagiK", "release-assets.json"],
            )
            for line in (output / "SHA256SUMS").read_text().splitlines():
                expected, name = line.split("  ", 1)
                self.assertNotIn("/", name)
                self.assertEqual(
                    expected, hashlib.sha256((output / name).read_bytes()).hexdigest()
                )

    def test_rejects_flattening_collisions(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            candidate = root / "candidate"
            (candidate / "one").mkdir(parents=True)
            (candidate / "two").mkdir()
            (candidate / "one/asset").write_bytes(b"one")
            (candidate / "two/asset").write_bytes(b"two")

            with self.assertRaisesRegex(ValueError, "collision"):
                module.prepare(candidate, root / "published")


if __name__ == "__main__":
    unittest.main()
