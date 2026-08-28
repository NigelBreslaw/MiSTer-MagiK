#!/usr/bin/env python3
# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

from __future__ import annotations

import importlib.util
import json
import tempfile
import unittest
from pathlib import Path

SCRIPT = (
    Path(__file__).resolve().parents[1] / "release/packaging/package-release-assets.py"
)
SPEC = importlib.util.spec_from_file_location("package_release_assets", SCRIPT)
assert SPEC and SPEC.loader
module = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(module)


class ReleaseAssetTests(unittest.TestCase):
    def test_flattens_tree_and_records_hashes(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            stage = root / "stage"
            (stage / "Scripts").mkdir(parents=True)
            (stage / "Scripts/MiSTer-MagiK.sh").write_bytes(b"installer")
            archive = root / "mister-magik-0.2.42.zip"
            archive.write_bytes(b"zip")
            output = root / "assets"

            module.build_assets(stage, archive, output, "0.2.42", 42)

            receipt = json.loads((output / "release-assets.json").read_text())
            self.assertEqual(receipt["version"], "0.2.42")
            self.assertEqual(receipt["build_number"], 42)
            self.assertEqual(receipt["files"][0]["path"], "Scripts/MiSTer-MagiK.sh")
            self.assertTrue(
                (output / "files/mister-magik--Scripts--MiSTer-MagiK.sh").is_file()
            )
            self.assertIn(
                "files/mister-magik--Scripts--MiSTer-MagiK.sh",
                (output / "SHA256SUMS").read_text(),
            )

    def test_rejects_mismatched_version(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            stage = root / "stage"
            stage.mkdir()
            archive = root / "archive.zip"
            archive.write_bytes(b"zip")
            with self.assertRaisesRegex(ValueError, "version/build mismatch"):
                module.build_assets(stage, archive, root / "out", "0.2.41", 42)


if __name__ == "__main__":
    unittest.main()
