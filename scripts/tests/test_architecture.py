# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

import subprocess
import tempfile
import unittest
from pathlib import Path
from typing import Any, cast

from scripts.magik_ci.architecture import report


class ArchitectureTests(unittest.TestCase):
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
            host = root / "agent-cli/src/host"
            host.mkdir(parents=True)
            body = "fn work() {\n    let mut state = 0;\n    state += 1;\n}\n"
            (host / "mod.rs").write_text(body)
            git("add", "--", "agent-cli")
            git("commit", "-qm", "baseline")
            base = git("rev-parse", "HEAD")
            (host / "mod.rs").write_text("mod delivery;\n")
            (host / "delivery.rs").write_text(body)
            git("add", "--", "agent-cli")
            git("commit", "-qm", "extract")
            before = cast(dict[str, Any], report(root, base, base))
            after = cast(dict[str, Any], report(root, base, "HEAD"))
            old = next(
                item
                for item in before["hotspots"]
                if item["owner_id"] == "host-workflows"
            )
            new = next(
                item
                for item in after["hotspots"]
                if item["owner_id"] == "host-workflows"
            )
            self.assertEqual(new["file_lines"], 1)
            self.assertEqual(new["subsystem"]["lines"], old["subsystem"]["lines"] + 1)
            self.assertEqual(new["subsystem"]["mutable_binding_count"], 1)
            self.assertEqual(
                new["subsystem"]["largest_function"]["path"],
                "agent-cli/src/host/delivery.rs",
            )
            self.assertEqual(len(after["hotspots"]), 6)
            missing = next(
                item for item in after["hotspots"] if item["owner_id"] == "device-agent"
            )
            self.assertFalse(missing["present"])
            self.assertEqual(missing["subsystem"]["file_count"], 0)


if __name__ == "__main__":
    unittest.main()
