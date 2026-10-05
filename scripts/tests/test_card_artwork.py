# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

import json
import tempfile
import unittest
from pathlib import Path

from scripts.magik_ci import card_artwork
from scripts.magik_ci.distribution import ROOT
from scripts.tests.distribution_fixture import CandidateFixture
from scripts.magik_ci import distribution as dist


class CardArtworkTests(unittest.TestCase):
    def test_shipped_pack_has_all_approved_sources_and_taxonomy_keys(self):
        root = ROOT / "apps/mister" / card_artwork.RELATIVE_PATH
        files = card_artwork.validate(root)
        self.assertEqual(
            len(files), 101
        )  # 32 unchanged root/system images + 15 shared family images + index
        self.assertEqual(
            files,
            {
                p.name
                for p in root.iterdir()
                if p.suffix in (".rgb888", ".cardtex", ".json")
            },
        )
        index = json.loads((root / "index.json").read_text())["cards"]
        taxonomy = json.loads(
            (ROOT / "crates/catalog/data/system_taxonomy.json").read_text()
        )
        systems = {s["id"] for s in taxonomy["systems"]}
        aliases = {a for s in taxonomy["systems"] for a in s.get("aliases", [])}
        self.assertLessEqual({k for k in index if ":" not in k}, systems | aliases)
        self.assertEqual(index["spectrum"], index["zx-spectrum"])
        for key in (
            "nes",
            "n64",
            "snes",
            "saturn",
            "megadrive",
            "x68000",
            "fmtowns",
            "menu:consoles:sega",
            "menu:computers:japanese",
        ):
            self.assertIn(key, index)

    def test_wordmark_metadata_is_boolean_and_family_sources_are_shared(self):
        root = ROOT / "apps/mister" / card_artwork.RELATIVE_PATH
        index = json.loads((root / "index.json").read_text())["cards"]
        self.assertEqual(
            {
                k: v
                for k, v in index["menu:consoles:nintendo"].items()
                if k != "prepared"
            },
            {
                k: v
                for k, v in index["menu:handhelds:nintendo"].items()
                if k != "prepared"
            },
        )
        self.assertTrue(index["menu:consoles:nintendo"]["contains_name"])
        self.assertFalse(index["menu:computers:commodore"]["contains_name"])
        with tempfile.TemporaryDirectory() as temp:
            fixture = CandidateFixture(Path(temp))
            staged = fixture.stage / dist.APP / card_artwork.RELATIVE_PATH
            manifest = json.loads((staged / "index.json").read_text())
            manifest["cards"]["root:arcade"]["contains_name"] = "true"
            (staged / "index.json").write_text(json.dumps(manifest))
            with self.assertRaisesRegex(ValueError, "name policy"):
                card_artwork.validate(staged)

    def test_missing_truncated_tampered_and_traversing_sources_fail_packaging(self):
        for damage in ("missing", "truncated", "checksum", "traversal"):
            with self.subTest(damage=damage), tempfile.TemporaryDirectory() as temp:
                fixture = CandidateFixture(Path(temp))
                root = fixture.stage / dist.APP / card_artwork.RELATIVE_PATH
                source = root / "fixture.rgb888"
                if damage == "missing":
                    source.unlink()
                elif damage == "truncated":
                    source.write_bytes(b"partial")
                elif damage == "checksum":
                    source.write_bytes(b"x" * card_artwork.SOURCE_BYTES)
                else:
                    index = json.loads((root / "index.json").read_text())
                    index["cards"]["root:arcade"]["file"] = "../fixture.rgb888"
                    (root / "index.json").write_text(json.dumps(index))
                with self.assertRaises(ValueError):
                    card_artwork.validate(root)
