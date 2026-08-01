#!/usr/bin/env python3
# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

from __future__ import annotations

import json
import tempfile
import unittest
import zipfile
from importlib.util import module_from_spec, spec_from_file_location
from pathlib import Path

SCRIPT = (
    Path(__file__).resolve().parents[1]
    / "release/packaging/verify-release-identity.py"
)
SPEC = spec_from_file_location("verify_release_identity", SCRIPT)
assert SPEC and SPEC.loader
MODULE = module_from_spec(SPEC)
SPEC.loader.exec_module(MODULE)


class ReleaseIdentityTests(unittest.TestCase):
    def alpha_assets(self, root: Path) -> None:
        version = "0.2.42"
        archive = root / f"mister-magik-{version}.zip"
        with zipfile.ZipFile(archive, "w") as package:
            package.writestr(
                "mister-magik/release-v1.txt",
                "version=0.2.42\nbuild_number=42\ngame_database_version=3\n",
            )
            package.writestr("mister-magik/mister-magik-fb", b"binary 0.2.42")
            package.writestr(
                "mister-magik/game-databases-manifest.json",
                json.dumps({"release_version": 3}),
            )
        (root / "release-assets.json").write_text(
            json.dumps(
                {
                    "version": version,
                    "build_number": 42,
                    "archive": archive.name,
                }
            )
        )
        (root / "mister-magik-alpha-db.json").write_text(
            json.dumps(
                {
                    "db_id": "mister_magik",
                    "release": {"version": version, "build_number": 42},
                    "url": (
                        "https://example.invalid/releases/download/"
                        "alpha-candidate-v0.2.42-012345abcdef/asset"
                    ),
                }
            )
        )

    def test_alpha_release_and_candidate_tags_have_distinct_roles(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            self.alpha_assets(root)
            MODULE.verify(
                root,
                "0.2.42",
                42,
                "alpha",
                "alpha",
                "alpha-candidate-v0.2.42-012345abcdef",
            )

    def test_alpha_database_must_reference_candidate(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            self.alpha_assets(root)
            database = root / "mister-magik-alpha-db.json"
            database.write_text(
                database.read_text().replace("012345abcdef", "fedcba543210")
            )
            with self.assertRaisesRegex(ValueError, "candidate tag"):
                MODULE.verify(
                    root,
                    "0.2.42",
                    42,
                    "alpha",
                    "alpha",
                    "alpha-candidate-v0.2.42-012345abcdef",
                )


if __name__ == "__main__":
    unittest.main()
