# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

import subprocess
import tempfile
import unittest
from pathlib import Path
from typing import Any, cast

from scripts.magik_ci.architecture import _concentration_source, report


class ArchitectureTests(unittest.TestCase):
    def test_concentration_excludes_generated_and_archived_sources(self):
        for path in (
            "history/log.tsv",
            "reference/Main/main.cpp",
            "private/assets/data.json",
            "build/output.rs",
            "apps/mister/target/output.rs",
            "mister/platform/contracts/generated/source.rs",
            "apps/mister/ui-generated/src/lib.rs",
            "crates/wire/src/generated.rs",
            "mister/platform/fpga/protocol.svh",
            "vendor/dependency/lib.rs",
        ):
            with self.subTest(path=path):
                self.assertFalse(_concentration_source(path))
        self.assertTrue(_concentration_source("apps/mister/src/ui_runner.rs"))

    def test_extraction_keeps_complete_family_visible(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)

            def git(*args):
                return (
                    subprocess.check_output(["git", "-C", str(root), *args])
                    .decode()
                    .strip()
                )

            git("init", "-q")
            git("config", "user.name", "Fixture")
            git("config", "user.email", "fixture@example.invalid")
            host = root / "apps/desktop/src"
            host.mkdir(parents=True)
            body = "fn work() {\n    let mut state = 0;\n    state += 1;\n}\n"
            (host / "main.rs").write_text(body)
            git("add", "--", "apps/desktop")
            git("commit", "-qm", "baseline")
            base = git("rev-parse", "HEAD")
            (host / "main.rs").write_text("mod delivery;\n")
            (host / "delivery.rs").write_text(body)
            git("add", "--", "apps/desktop")
            git("commit", "-qm", "extract")
            before = cast(dict[str, Any], report(root, base, base))
            after = cast(dict[str, Any], report(root, base, "HEAD"))
            old = next(
                item for item in before["hotspots"] if item["owner_id"] == "desktop-app"
            )
            new = next(
                item for item in after["hotspots"] if item["owner_id"] == "desktop-app"
            )
            self.assertEqual(new["file_lines"], 1)
            self.assertEqual(new["subsystem"]["lines"], old["subsystem"]["lines"] + 1)
            self.assertEqual(new["subsystem"]["mutable_binding_count"], 1)
            self.assertNotIn("largest_function", new["subsystem"])
            self.assertEqual(len(after["hotspots"]), 3)
            self.assertEqual(after["schema"], "mister-magik-architecture-report-v2")
            missing = next(
                item
                for item in after["hotspots"]
                if item["owner_id"] == "launcher-runtime"
            )
            self.assertFalse(missing["present"])
            self.assertEqual(missing["subsystem"]["file_count"], 0)


if __name__ == "__main__":
    unittest.main()
