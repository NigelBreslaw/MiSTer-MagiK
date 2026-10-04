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
            len(files), 43
        )  # six root + thirteen console + 23 computer, plus index
        self.assertEqual(
            files, {p.name for p in root.iterdir() if p.suffix in (".rgb888", ".json")}
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
