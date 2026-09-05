# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

import json
import os
import shutil
import subprocess
import tempfile
import unittest
from pathlib import Path

from scripts.magik_ci.guidance import render, report

ROOT = Path(__file__).resolve().parents[2]


class GuidanceTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.root = Path(self.temporary.name).resolve()
        self.write("AGENTS.md", "Root rules")
        self.write("apps/AGENTS.md", "Application rules")
        self.write("apps/mister/AGENTS.md", "Device UI rules")

    def tearDown(self):
        self.temporary.cleanup()

    def write(self, path, contents):
        target = self.root / path
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_text(contents)
        return target

    def test_new_file_receives_all_existing_ancestors(self):
        result = report(self.root, Path("apps/mister/new/deep.rs"))
        self.assertEqual(
            result["guidance"], ["AGENTS.md", "apps/AGENTS.md", "apps/mister/AGENTS.md"]
        )
        self.assertIn("global", result["guidance_scope"])
        self.assertIn(
            "guidance: AGENTS.md, apps/AGENTS.md, apps/mister/AGENTS.md", render(result)
        )

    def test_override_empty_files_fallback_and_removed_scope(self):
        self.write("apps/AGENTS.override.md", "Override")
        self.write("apps/mister/AGENTS.override.md", "  ")
        self.write(".codex/config.toml", 'project_doc_fallback_filenames = ["TEAM.md"]')
        self.write("apps/mister/TEAM.md", "Team rules")
        (self.root / "apps/mister/AGENTS.md").unlink()
        self.assertEqual(
            report(self.root, Path("apps/mister"))["guidance"],
            ["AGENTS.md", "apps/AGENTS.override.md", "apps/mister/TEAM.md"],
        )

    def test_absolute_and_directory_paths(self):
        self.assertEqual(
            report(self.root, self.root / "apps")["guidance"],
            ["AGENTS.md", "apps/AGENTS.md"],
        )
        self.assertEqual(report(self.root, Path("."))["guidance"], ["AGENTS.md"])

    def test_escapes_and_symlinks_are_rejected(self):
        with tempfile.TemporaryDirectory() as outside:
            (self.root / "outside").symlink_to(outside, target_is_directory=True)
            for path in [
                Path("../file.rs"),
                Path(outside) / "new.rs",
                Path("outside/new.rs"),
            ]:
                with (
                    self.subTest(path=path),
                    self.assertRaisesRegex(ValueError, "guidance_path_"),
                ):
                    report(self.root, path)

    def test_device_state_is_classified_without_device_access(self):
        for path in [
            "/media/fat",
            "/media/fat/mister-magik/launcher.env",
            "/tmp/mister-magik/fs-fault.json",
        ]:
            with self.subTest(path=path):
                result = report(self.root, Path(path))
                self.assertEqual(
                    result["authority"],
                    "device-owned runtime state; never copy into Git",
                )
                self.assertEqual(result["guidance"], ["AGENTS.md"])
        with self.assertRaises(ValueError):
            report(self.root, Path("/media/fat-unrelated/file"))

    def test_regeneration_and_reference_mappings(self):
        result = report(self.root, Path("docs/reference/mister-runtime-environment.md"))
        self.assertEqual(
            result["regeneration"],
            "python3 scripts/checks/generate-runtime-environment-reference.py",
        )
        self.assertEqual(
            report(self.root, Path("mister/platform/contracts/generated/source.rs"))[
                "authority"
            ],
            "checked-in generated platform-v3 consumer; never hand-edit",
        )
        self.assertEqual(
            report(self.root, Path("crates/catalog/src/query.rs"))["canonical"],
            "matching heading in docs/catalog.md",
        )

    def test_invalid_fallback_cannot_escape_repository(self):
        self.write(
            ".codex/config.toml", 'project_doc_fallback_filenames = ["../TEAM.md"]'
        )
        with self.assertRaisesRegex(ValueError, "guidance_invalid_fallback"):
            report(self.root, Path("apps/file.rs"))

    def test_wrapper_is_bootstrap_free_and_json_is_clean(self):
        for relative in [
            "scripts/agent",
            "scripts/lib/shared-worktree-cache.sh",
            "scripts/magik_ci/guidance.py",
        ]:
            target = self.root / relative
            target.parent.mkdir(parents=True, exist_ok=True)
            shutil.copy2(ROOT / relative, target)
        for name in ["cargo", "rustc"]:
            command = self.write("bin/" + name, "#!/bin/sh\nexit 99\n")
            command.chmod(0o755)
        result = subprocess.run(
            [
                "bash",
                str(self.root / "scripts/agent"),
                "guidance",
                "apps/mister/new.rs",
                "--json",
            ],
            env={
                **os.environ,
                "PATH": str(self.root / "bin") + os.pathsep + os.environ["PATH"],
            },
            capture_output=True,
            text=True,
            check=True,
        )
        self.assertEqual(
            json.loads(result.stdout)["guidance"],
            ["AGENTS.md", "apps/AGENTS.md", "apps/mister/AGENTS.md"],
        )
        self.assertEqual(result.stderr, "")
        self.assertFalse((self.root / "agent-cli/target").exists())


if __name__ == "__main__":
    unittest.main()
