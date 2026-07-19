#!/usr/bin/env python3
# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

from __future__ import annotations

import importlib.util
import json
import sqlite3
import tempfile
import unittest
import zipfile
from contextlib import closing
from pathlib import Path

SCRIPT = Path(__file__).resolve().parents[1] / "release/databases/game-databases-bundle.py"
SPEC = importlib.util.spec_from_file_location("game_databases_bundle", SCRIPT)
assert SPEC and SPEC.loader
bundle = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(bundle)


def build_mame(path: Path, tag: str = "mame0288") -> None:
    with closing(sqlite3.connect(path)) as database, database:
        database.executescript(
            f"""
            CREATE TABLE mame_machines(
              setname TEXT PRIMARY KEY,
              parent_setname TEXT,
              title TEXT NOT NULL,
              players INTEGER,
              control_type TEXT,
              source_version TEXT NOT NULL
            ) WITHOUT ROWID;
            WITH RECURSIVE seq(i) AS (
              VALUES(1) UNION ALL SELECT i + 1 FROM seq WHERE i < 50000
            )
            INSERT INTO mame_machines
            SELECT 'machine' || i, '', 'Machine ' || i, 1 + (i % 4), 'joy',
                   '0.288 ({tag})' FROM seq;
            CREATE TABLE mame_software_items(list_name TEXT NOT NULL, item_name TEXT NOT NULL);
            INSERT INTO mame_software_items VALUES
              ('lynx','one'),('megadriv','one'),('n64','one'),('nes','one'),('saturn','one'),('sms','one'),('snes','one');
            """
        )


def build_hbmame(path: Path) -> None:
    with closing(sqlite3.connect(path)) as database, database:
        database.executescript(
            """
            CREATE TABLE mame_machines(
              setname TEXT PRIMARY KEY,
              parent_setname TEXT,
              title TEXT NOT NULL,
              players INTEGER,
              control_type TEXT
            ) WITHOUT ROWID;
            WITH RECURSIVE seq(i) AS (
              VALUES(1) UNION ALL SELECT i + 1 FROM seq WHERE i < 5000
            )
            INSERT INTO mame_machines
            SELECT 'machine' || i, '', 'Machine ' || i, 1 + (i % 4), 'joy' FROM seq;
            INSERT INTO mame_machines VALUES('marpy','mappy','Marpy',2,'joy');
            """
        )


class GameDatabaseBundleTests(unittest.TestCase):
    def setUp(self) -> None:
        self.temporary = tempfile.TemporaryDirectory(prefix="game-databases-bundle-")
        self.root = Path(self.temporary.name)
        self.mame = self.root / "mame.sqlite3"
        self.hbmame = self.root / "hbmame.sqlite3"
        build_mame(self.mame)
        build_hbmame(self.hbmame)

    def tearDown(self) -> None:
        self.temporary.cleanup()

    def args(self):
        return type("Args", (), {
            "mame_sqlite": self.mame,
            "hbmame_sqlite": self.hbmame,
            "release_version": 1,
            "mame_tag": "mame0288",
            "mame_sha": "1" * 40,
            "mame_listxml_asset": "mame0288lx.zip",
            "mame_listxml_sha256": "5" * 64,
            "hbmame_tag": "tag24532",
            "hbmame_sha": "2" * 40,
            "mame_builder_sha": "3" * 40,
            "hbmame_builder_sha": "4" * 40,
            "output": self.root / "output",
        })()

    def test_round_trip(self) -> None:
        archive = bundle.create(self.args())
        self.assertEqual(archive.name, "mister-magik-game-databases-v1.zip")
        payload = bundle.verify(
            archive,
            self.root / "output/game-databases-manifest.json",
            self.root / "output/SHA256SUMS",
        )
        self.assertEqual(payload["release_version"], 1)
        self.assertEqual(payload["sources"]["hbmame"]["tag"], "tag24532")
        extracted = self.root / "extracted"
        bundle.extract_release(self.root / "output", extracted)
        self.assertEqual(
            set(path.name for path in extracted.iterdir()),
            {"mame.sqlite3", "hbmame.sqlite3", "game-databases-manifest.json", "SHA256SUMS"},
        )

    def test_release_directory_requires_external_checksums(self) -> None:
        bundle.create(self.args())
        (self.root / "output/SHA256SUMS").unlink()
        with self.assertRaisesRegex(ValueError, "manifest or checksums"):
            bundle.extract_release(self.root / "output", self.root / "extracted")

    def test_release_directory_rejects_ambiguous_archives(self) -> None:
        archive = bundle.create(self.args())
        (self.root / "output/mister-magik-game-databases-v2.zip").write_bytes(archive.read_bytes())
        with self.assertRaisesRegex(ValueError, "exactly its numbered archive"):
            bundle.extract_release(self.root / "output", self.root / "extracted")

    def test_release_directory_rejects_missing_archive(self) -> None:
        archive = bundle.create(self.args())
        archive.unlink()
        with self.assertRaisesRegex(ValueError, "exactly its numbered archive"):
            bundle.extract_release(self.root / "output", self.root / "extracted")

    def test_release_directory_rejects_corrupt_archive(self) -> None:
        archive = bundle.create(self.args())
        archive.write_bytes(b"not a zip archive")
        with self.assertRaises(zipfile.BadZipFile):
            bundle.extract_release(self.root / "output", self.root / "extracted")

    def test_release_directory_rejects_external_checksum_disagreement(self) -> None:
        bundle.create(self.args())
        (self.root / "output/SHA256SUMS").write_text("tampered\n")
        with self.assertRaisesRegex(ValueError, "checksums differ"):
            bundle.extract_release(self.root / "output", self.root / "extracted")

    def test_tampered_reused_database_is_rejected(self) -> None:
        archive = bundle.create(self.args())
        altered_dir = self.root / "altered"
        altered_dir.mkdir()
        altered = altered_dir / archive.name
        with zipfile.ZipFile(archive) as source, zipfile.ZipFile(altered, "w") as target:
            for info in source.infolist():
                content = source.read(info.filename)
                if info.filename == "hbmame.sqlite3":
                    content += b"tampered"
                target.writestr(info, content)
        with self.assertRaisesRegex(ValueError, "manifest|checksum"):
            bundle.verify(altered)

    def test_undersized_database_is_rejected(self) -> None:
        undersized = self.root / "undersized.sqlite3"
        with closing(sqlite3.connect(undersized)) as database, database:
            database.execute(
                "CREATE TABLE mame_machines(setname TEXT PRIMARY KEY, parent_setname TEXT, title TEXT)"
            )
        with self.assertRaisesRegex(ValueError, "too few machine rows"):
            bundle.validate_database(undersized, "HBMAME")

    def test_corrupt_database_is_rejected(self) -> None:
        corrupt = self.root / "corrupt.sqlite3"
        corrupt.write_bytes(b"not sqlite")
        with self.assertRaisesRegex(ValueError, "invalid HBMAME SQLite database"):
            bundle.validate_database(corrupt, "HBMAME")

    def test_machine_metadata_columns_are_required(self) -> None:
        missing = self.root / "missing-metadata.sqlite3"
        with closing(sqlite3.connect(missing)) as database, database:
            database.executescript(
                """
                CREATE TABLE mame_machines(
                  setname TEXT PRIMARY KEY,
                  parent_setname TEXT,
                  title TEXT NOT NULL
                ) WITHOUT ROWID;
                WITH RECURSIVE seq(i) AS (
                  VALUES(1) UNION ALL SELECT i + 1 FROM seq WHERE i < 5000
                )
                INSERT INTO mame_machines
                SELECT 'machine' || i, '', 'Machine ' || i FROM seq;
                INSERT INTO mame_machines VALUES('marpy','mappy','Marpy');
                """
            )
        with self.assertRaisesRegex(ValueError, "missing machine metadata columns"):
            bundle.validate_database(missing, "HBMAME")

    def test_mame_source_mismatch_is_rejected(self) -> None:
        mismatched = self.root / "mismatched.sqlite3"
        build_mame(mismatched, "mame0287")
        with self.assertRaisesRegex(ValueError, "source version does not match"):
            bundle.validate_database(mismatched, "MAME", "mame0288")

    def test_external_manifest_disagreement_is_rejected(self) -> None:
        archive = bundle.create(self.args())
        manifest = self.root / "different.json"
        payload = json.loads((self.root / "output/game-databases-manifest.json").read_text())
        payload["release_version"] = 2
        manifest.write_text(json.dumps(payload))
        with self.assertRaisesRegex(ValueError, "differs"):
            bundle.verify(archive, manifest)

    def test_external_manifest_byte_tampering_is_rejected(self) -> None:
        archive = bundle.create(self.args())
        manifest = self.root / "output/game-databases-manifest.json"
        manifest.write_text(manifest.read_text() + "\n")
        with self.assertRaisesRegex(ValueError, "differs"):
            bundle.verify(archive, manifest, self.root / "output/SHA256SUMS")

    def test_archive_traversal_is_rejected(self) -> None:
        unsafe = self.root / "mister-magik-game-databases-v1.zip"
        with zipfile.ZipFile(unsafe, "w") as archive:
            archive.writestr("../mame.sqlite3", "unsafe")
        with self.assertRaisesRegex(ValueError, "unsafe"):
            bundle.verify(unsafe)

    def test_update_plan_covers_initial_and_partial_updates(self) -> None:
        initial = bundle.update_plan(None, "mame0288", "1" * 40, "tag24532", "2" * 40)
        self.assertEqual(initial, {
            "current_version": 0,
            "next_version": 1,
            "mame_changed": True,
            "hbmame_changed": True,
            "update_needed": True,
        })
        archive = bundle.create(self.args())
        current = bundle.verify(archive)
        same = bundle.update_plan(current, "mame0288", "1" * 40, "tag24532", "2" * 40)
        self.assertFalse(same["update_needed"])
        self.assertEqual(same["next_version"], 2)
        mame = bundle.update_plan(current, "mame0289", "4" * 40, "tag24532", "2" * 40)
        self.assertTrue(mame["mame_changed"])
        self.assertFalse(mame["hbmame_changed"])
        hbmame = bundle.update_plan(current, "mame0288", "1" * 40, "tag24533", "5" * 40)
        self.assertFalse(hbmame["mame_changed"])
        self.assertTrue(hbmame["hbmame_changed"])
        both = bundle.update_plan(current, "mame0289", "6" * 40, "tag24533", "7" * 40)
        self.assertTrue(both["mame_changed"])
        self.assertTrue(both["hbmame_changed"])
        self.assertTrue(both["update_needed"])


if __name__ == "__main__":
    unittest.main()
