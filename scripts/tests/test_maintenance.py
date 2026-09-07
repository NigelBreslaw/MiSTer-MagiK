import json
import sqlite3
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

from scripts.magik_ci import maintenance


class MaintenanceTests(unittest.TestCase):
    def test_dependency_sync_does_not_globally_update(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            with (
                patch.object(
                    maintenance, "tracked_manifest", return_value=root / "Cargo.toml"
                ),
                patch.object(maintenance.subprocess, "run") as run,
            ):
                maintenance.dependencies(root, Path("Cargo.toml"))
            self.assertEqual(len(run.call_args_list), 2)
            self.assertTrue(
                all("metadata" in call.args[0] for call in run.call_args_list)
            )
            self.assertIn("--locked", run.call_args_list[-1].args[0])

    def test_evidence_export_preserves_rows_and_database(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            database = root / "old.sqlite"
            with sqlite3.connect(database) as connection:
                connection.execute("CREATE TABLE runs(id TEXT, elapsed INTEGER)")
                connection.execute("INSERT INTO runs VALUES('one', 42)")
            before = database.read_bytes()
            output = root / "export.json"
            maintenance.export_evidence(database, output)
            self.assertEqual(
                json.loads(output.read_text())["runs"]["rows"], [["one", 42]]
            )
            self.assertEqual(database.read_bytes(), before)
            with self.assertRaises(ValueError):
                maintenance.export_evidence(database, output)
