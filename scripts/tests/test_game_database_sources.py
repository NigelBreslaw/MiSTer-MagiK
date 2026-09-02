# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

"""Offline tests for the canonical SQLite source-restoration contract."""

from __future__ import annotations

import subprocess
import sys
import tempfile
import unittest
import zipfile
from pathlib import Path

from scripts.magik_ci.databases import restore_sources

ROOT = Path(__file__).resolve().parents[2]
DATABASES = {"mame.sqlite3": b"old mame", "hbmame.sqlite3": b"old hbmame"}


class DatabaseSourceTests(unittest.TestCase):
    def setUp(self) -> None:
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        self.release = self.root / "release.zip"
        self.source = self.root / "source.zip"
        self.output = self.root / "current"

    def archive(self, path: Path, files: dict[str, bytes]) -> None:
        with zipfile.ZipFile(path, "w") as stream:
            for name, data in files.items():
                stream.writestr(name, data)

    def assert_restored(self) -> None:
        for name, data in DATABASES.items():
            self.assertEqual((self.output / name).read_bytes(), data)
        self.assertFalse((self.output / "source").exists())

    def cli(self, *arguments: str) -> subprocess.CompletedProcess[str]:
        return subprocess.run(
            [
                sys.executable,
                str(ROOT / "scripts/magik-ci"),
                "ci",
                "game-databases",
                "restore-sources",
                str(self.release),
                "--output",
                str(self.output),
                *arguments,
            ],
            capture_output=True,
            text=True,
            timeout=10,
            check=False,
        )

    def test_compact_source_has_canonical_paths_and_preserves_other_files(self) -> None:
        self.archive(self.release, {"magik-metadata-v1.bin": b"compact"})
        self.archive(self.source, {**DATABASES, "../unexpected": b"ignored"})
        self.output.mkdir()
        marker = self.output / "game-databases-manifest.json"
        marker.write_bytes(b"preserve")
        restore_sources(self.release, self.output, self.source)
        self.assert_restored()
        self.assertEqual(marker.read_bytes(), b"preserve")
        self.assertFalse((self.root / "unexpected").exists())

    def test_legacy_fallback_with_missing_or_unspecified_source(self) -> None:
        self.archive(self.release, DATABASES)
        for source in (None, self.source):
            with self.subTest(source=source):
                restore_sources(self.release, self.output, source)
                self.assert_restored()

    def test_present_source_takes_precedence_over_legacy(self) -> None:
        self.archive(self.release, {name: b"other" for name in DATABASES})
        self.archive(self.source, DATABASES)
        restore_sources(self.release, self.output, self.source)
        self.assert_restored()

    def test_missing_or_empty_members_fail_before_writing_either(self) -> None:
        self.archive(self.release, DATABASES)
        for name in DATABASES:
            for empty in (False, True):
                with self.subTest(name=name, empty=empty):
                    files = dict(DATABASES)
                    if empty:
                        files[name] = b""
                    else:
                        del files[name]
                    self.archive(self.source, files)
                    with self.assertRaises(ValueError) as caught:
                        restore_sources(self.release, self.output, self.source)
                    self.assertIn(str(self.source), str(caught.exception))
                    self.assertIn(name, str(caught.exception))
                    self.assertFalse(self.output.exists())

    def test_malformed_present_source_does_not_fall_back(self) -> None:
        self.archive(self.release, DATABASES)
        self.source.write_bytes(b"not a zip")
        with self.assertRaises(ValueError) as caught:
            restore_sources(self.release, self.output, self.source)
        self.assertIn(str(self.source), str(caught.exception))
        self.assertFalse(self.output.exists())

    def test_corrupt_member_reports_name_before_writing_either(self) -> None:
        self.archive(self.source, DATABASES)
        # Stored ZIP members make a same-length payload change a CRC failure.
        self.source.write_bytes(
            self.source.read_bytes().replace(b"old hbmame", b"bad hbmame", 1)
        )
        with self.assertRaises(ValueError) as caught:
            restore_sources(self.release, self.output, self.source)
        self.assertIn(str(self.source), str(caught.exception))
        self.assertIn("hbmame.sqlite3", str(caught.exception))
        self.assertFalse(self.output.exists())

    def test_unavailable_sources_fail_without_output(self) -> None:
        self.archive(self.release, {"magik-metadata-v1.bin": b"compact"})
        with self.assertRaises(ValueError) as caught:
            restore_sources(self.release, self.output, self.source)
        self.assertIn(str(self.release), str(caught.exception))
        self.assertIn("mame.sqlite3", str(caught.exception))
        self.assertFalse(self.output.exists())

    def test_failed_validation_preserves_existing_databases(self) -> None:
        self.output.mkdir()
        for name in DATABASES:
            (self.output / name).write_bytes(b"keep")
        self.archive(self.source, {"mame.sqlite3": b"replacement"})
        with self.assertRaises(ValueError):
            restore_sources(self.release, self.output, self.source)
        for name in DATABASES:
            self.assertEqual((self.output / name).read_bytes(), b"keep")

    def test_cli_compact_and_legacy(self) -> None:
        self.archive(self.release, DATABASES)
        self.archive(self.source, DATABASES)
        for arguments in ((), ("--source-archive", str(self.source))):
            with self.subTest(arguments=arguments):
                result = self.cli(*arguments)
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assert_restored()

    def test_cli_reports_archive_and_member(self) -> None:
        self.archive(self.release, {"mame.sqlite3": b"mame"})
        result = self.cli()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn(str(self.release), result.stderr)
        self.assertIn("hbmame.sqlite3", result.stderr)
        self.assertFalse(self.output.exists())


if __name__ == "__main__":
    unittest.main()
