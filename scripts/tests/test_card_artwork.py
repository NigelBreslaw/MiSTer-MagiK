# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

import json
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

from scripts.magik_ci import card_artwork
from scripts.magik_ci.distribution import ROOT
from scripts.tests.distribution_fixture import CandidateFixture
from scripts.magik_ci import distribution as dist


class CardArtworkTests(unittest.TestCase):
    def test_staging_cli_copies_only_declared_artwork(self):
        for existing in (False, True):
            with self.subTest(existing=existing), tempfile.TemporaryDirectory() as temp:
                fixture = CandidateFixture(Path(temp))
                source = fixture.stage / dist.APP / card_artwork.RELATIVE_PATH
                (source / "unused.cardtex").write_bytes(b"undeclared artwork")
                destination = Path(temp) / "staged-artwork"
                if existing:
                    destination.mkdir()
                subprocess.run(
                    [
                        sys.executable,
                        str(ROOT / "scripts/magik-ci"),
                        "ci",
                        "distribution",
                        "stage-artwork",
                        str(source),
                        str(destination),
                    ],
                    cwd=temp,
                    check=True,
                    capture_output=True,
                    text=True,
                )
                expected = {"index.json", "fixture.rgb888", "fixture.cardtex"}
                self.assertEqual(
                    {path.name for path in destination.iterdir()}, expected
                )
                for name in expected:
                    self.assertEqual(
                        (destination / name).read_bytes(), (source / name).read_bytes()
                    )

    def test_nonempty_destination_is_preserved_and_rejected(self):
        with tempfile.TemporaryDirectory() as temp:
            fixture = CandidateFixture(Path(temp))
            source = fixture.stage / dist.APP / card_artwork.RELATIVE_PATH
            destination = Path(temp) / "staged-artwork"
            destination.mkdir()
            leftover = destination / "fixture.cardtex"
            leftover.write_bytes(b"existing artwork")
            with self.assertRaisesRegex(ValueError, "destination must be"):
                card_artwork.stage(source, destination)
            self.assertEqual(list(destination.iterdir()), [leftover])
            self.assertEqual(leftover.read_bytes(), b"existing artwork")

    def test_file_or_symlink_destination_is_preserved_and_rejected(self):
        for kind in ("file", "symlink"):
            with self.subTest(kind=kind), tempfile.TemporaryDirectory() as temp:
                fixture = CandidateFixture(Path(temp))
                source = fixture.stage / dist.APP / card_artwork.RELATIVE_PATH
                destination = Path(temp) / "staged-artwork"
                target = Path(temp) / "target"
                if kind == "file":
                    destination.write_bytes(b"existing file")
                else:
                    target.mkdir()
                    destination.symlink_to(target, target_is_directory=True)
                with self.assertRaisesRegex(ValueError, "destination must be"):
                    card_artwork.stage(source, destination)
                if kind == "file":
                    self.assertEqual(destination.read_bytes(), b"existing file")
                else:
                    self.assertTrue(destination.is_symlink())
                    self.assertEqual(list(target.iterdir()), [])

    def test_invalid_prepared_declarations_fail_before_staging(self):
        for damage in ("traversal", "absolute", "extension", "digest", "size"):
            with self.subTest(damage=damage), tempfile.TemporaryDirectory() as temp:
                fixture = CandidateFixture(Path(temp))
                source = fixture.stage / dist.APP / card_artwork.RELATIVE_PATH
                index_path = source / "index.json"
                index = json.loads(index_path.read_text())
                prepared = index["cards"]["root:arcade"]["prepared"]
                if damage in ("traversal", "absolute", "extension"):
                    prepared["file"] = {
                        "traversal": "../fixture.cardtex",
                        "absolute": "/fixture.cardtex",
                        "extension": "fixture.rgb888",
                    }[damage]
                elif damage == "digest":
                    prepared["sha256"] = "0" * 64
                else:
                    prepared["bytes"] += 1
                index_path.write_text(json.dumps(index))
                destination = Path(temp) / "staged-artwork"
                with self.assertRaisesRegex(ValueError, "invalid.*source|conflicting"):
                    card_artwork.stage(source, destination)
                self.assertFalse(destination.exists())

    def test_identical_shared_artwork_is_copied_once(self):
        with tempfile.TemporaryDirectory() as temp:
            fixture = CandidateFixture(Path(temp))
            source = fixture.stage / dist.APP / card_artwork.RELATIVE_PATH
            with patch.object(
                card_artwork.shutil, "copyfile", wraps=card_artwork.shutil.copyfile
            ) as copyfile:
                card_artwork.stage(source, Path(temp) / "staged-artwork")
            self.assertEqual(copyfile.call_count, 3)
            self.assertEqual(
                {call.args[0].name for call in copyfile.call_args_list},
                {"index.json", "fixture.rgb888", "fixture.cardtex"},
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
