# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

import json
import os
import subprocess
import tempfile
import unittest
import zipfile
from pathlib import Path
from unittest.mock import patch

from scripts.magik_ci import card_artwork
from scripts.magik_ci.distribution import ROOT
from scripts.tests.distribution_fixture import CandidateFixture
from scripts.magik_ci import distribution as dist


class CardArtworkTests(unittest.TestCase):
    def test_distribution_script_stages_all_declared_artwork_in_zip(self):
        packaging = (ROOT / "scripts/package-distribution.sh").read_text()
        block = packaging.split("# Card artwork is mandatory runtime data", 1)[1]
        block = block.split("\n", 1)[1].split('\nif [[ -n "$ASSET_PACK" ]]', 1)[0]
        source = ROOT / "apps/mister" / card_artwork.RELATIVE_PATH
        expected = card_artwork.validate(source)
        with tempfile.TemporaryDirectory() as temp:
            stage = Path(temp) / "stage"
            subprocess.run(
                ["bash", "-ec", block],
                env={
                    **os.environ,
                    "ROOT": str(ROOT),
                    "STAGE": str(stage),
                    "PUBLIC_ROOT_RELATIVE": dist.APP,
                },
                check=True,
                capture_output=True,
                text=True,
            )
            archive = Path(temp) / "distribution.zip"
            subprocess.run(["zip", "-qr", str(archive), "."], cwd=stage, check=True)
            prefix = f"{dist.APP}/{card_artwork.RELATIVE_PATH}/"
            with zipfile.ZipFile(archive) as package:
                self.assertEqual(
                    {
                        entry.filename
                        for entry in package.infolist()
                        if not entry.is_dir()
                    },
                    {prefix + name for name in expected},
                )
                for name in expected:
                    self.assertEqual(
                        package.read(prefix + name), (source / name).read_bytes()
                    )

    def test_invalid_prepared_textures_fail_staging(self):
        for damage in ("missing", "truncated", "checksum"):
            with self.subTest(damage=damage), tempfile.TemporaryDirectory() as temp:
                fixture = CandidateFixture(Path(temp))
                source = fixture.stage / dist.APP / card_artwork.RELATIVE_PATH
                texture = source / "fixture.cardtex"
                if damage == "missing":
                    texture.unlink()
                elif damage == "truncated":
                    texture.write_bytes(b"partial")
                else:
                    texture.write_bytes(b"x" * texture.stat().st_size)
                destination = Path(temp) / "staged-artwork"
                with self.assertRaisesRegex(ValueError, "card artwork"):
                    card_artwork.stage(source, destination)
                self.assertFalse(destination.exists())

    def test_staging_rejects_prepared_texture_corrupted_during_copy(self):
        copyfile = card_artwork.shutil.copyfile

        def corrupt_copy(source, destination):
            copyfile(source, destination)
            if source.suffix == ".cardtex":
                destination.write_bytes(b"partial")

        with tempfile.TemporaryDirectory() as temp:
            fixture = CandidateFixture(Path(temp))
            source = fixture.stage / dist.APP / card_artwork.RELATIVE_PATH
            with patch.object(card_artwork.shutil, "copyfile", corrupt_copy):
                with self.assertRaisesRegex(ValueError, "invalid card artwork file"):
                    card_artwork.stage(source, Path(temp) / "staged-artwork")

    def test_shipped_pack_has_all_approved_sources_and_taxonomy_keys(self):
        root = ROOT / "apps/mister" / card_artwork.RELATIVE_PATH
        files = card_artwork.validate(root)
        self.assertEqual(len(files), 79)  # 36 sources + 42 prepared faces + index
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
            "x68000",
            "fmtowns",
            "menu:consoles:sega",
            "menu:computers:japanese",
        ):
            self.assertIn(key, index)
        for key in (
            "fds",
            "megacd",
            "megadrive",
            "n64",
            "nes",
            "s32x",
            "satellaview",
            "saturn",
            "sg1000",
            "sms",
            "snes",
        ):
            self.assertNotIn(key, index)
        self.assertFalse(any(name.startswith("console-") for name in files))

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
