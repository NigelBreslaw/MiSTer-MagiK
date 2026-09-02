#!/usr/bin/env python3
# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

from __future__ import annotations

import hashlib
import json
import tempfile
import unittest
import zipfile
from importlib.util import module_from_spec, spec_from_file_location
from pathlib import Path

SCRIPT = (
    Path(__file__).resolve().parents[1] / "release/databases/generate-downloader-db.py"
)
SPEC = spec_from_file_location("generate_downloader_db", SCRIPT)
assert SPEC and SPEC.loader
MODULE = module_from_spec(SPEC)
SPEC.loader.exec_module(MODULE)


class DownloaderDatabaseTests(unittest.TestCase):
    def test_channels_reference_their_release_assets(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            files = root / "files"
            files.mkdir()
            asset = files / "mister-magik--Scripts--MiSTer-MagiK.sh"
            asset.write_bytes(b"installer\n")
            receipt = {
                "format": "mister-magik-release-assets-v1",
                "version": "0.2.42",
                "build_number": 42,
                "files": [
                    {
                        "path": "Scripts/MiSTer-MagiK.sh",
                        "asset": asset.name,
                        "size": asset.stat().st_size,
                        "md5": hashlib.md5(asset.read_bytes()).hexdigest(),
                        "sha256": hashlib.sha256(asset.read_bytes()).hexdigest(),
                    }
                ],
            }
            receipt_path = root / "release-assets.json"
            receipt_path.write_text(json.dumps(receipt))

            cases = (
                ("alpha", "alpha", "v0.2.42", False),
                ("promoted-beta", "beta", "v0.2.42", True),
                ("release", "release", "v0.2.42", True),
            )
            for case, channel, tag, has_installer in cases:
                output = root / case
                MODULE.generate(
                    receipt_path, output, channel, "Owner/Repo", tag, 1_700_000_000
                )
                database = json.loads(
                    (output / f"mister-magik-{channel}-db.json").read_text()
                )
                self.assertEqual(database["v"], 1)
                self.assertEqual(database["db_id"], "mister_magik")
                self.assertEqual(database["timestamp"], 1_700_000_000)
                self.assertEqual(database["release"]["version"], "0.2.42")
                self.assertEqual(database["release"]["build_number"], 42)
                database_text = json.dumps(database).lower()
                self.assertNotIn("reboot", database_text)
                self.assertNotIn("restart", database_text)
                item = database["files"]["Scripts/MiSTer-MagiK.sh"]
                self.assertEqual(item["hash"], receipt["files"][0]["md5"])
                self.assertIn(f"/releases/download/{tag}/", item["url"])
                installer = output / f"mister-magik-{channel}-installer.zip"
                self.assertEqual(installer.exists(), has_installer)
                self.assertFalse((output / "downloader_mister_magik.ini").exists())
                if has_installer:
                    with zipfile.ZipFile(installer) as archive:
                        names = archive.namelist()
                        self.assertEqual(names, ["downloader_mister_magik.ini"])
                        ini = archive.read(names[0]).decode()
                        self.assertIn(f"mister-magik-{channel}-db.json.zip", ini)
                        self.assertNotIn("reboot", ini.lower())
                        self.assertNotIn("restart", ini.lower())
                self.assertNotIn("downloader_mister_magik.ini", database["files"])

    def test_rejects_forbidden_owned_path_and_version_mismatch(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            (root / "files").mkdir()
            asset = root / "files" / "bad"
            asset.write_bytes(b"bad")
            receipt = root / "release-assets.json"
            base = {
                "format": "mister-magik-release-assets-v1",
                "version": "0.2.7",
                "build_number": 7,
                "files": [
                    {"path": "MiSTer.ini", "asset": "bad", "size": 3, "md5": "x"}
                ],
            }
            receipt.write_text(json.dumps(base))
            with self.assertRaisesRegex(ValueError, "forbidden"):
                MODULE.generate(
                    receipt, root / "out", "beta", "Owner/Repo", "v0.2.7", 1_700_000_000
                )
            with self.assertRaisesRegex(ValueError, "disagree"):
                MODULE.generate(
                    receipt,
                    root / "out",
                    "release",
                    "Owner/Repo",
                    "v0.2.8",
                    1_700_000_000,
                )
            with self.assertRaisesRegex(ValueError, "disagree"):
                MODULE.generate(
                    receipt,
                    root / "out",
                    "alpha",
                    "Owner/Repo",
                    "alpha",
                    1_700_000_000,
                )


if __name__ == "__main__":
    unittest.main()
