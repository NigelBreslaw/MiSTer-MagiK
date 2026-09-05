# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

import json
import os
import subprocess
import tempfile
import unittest
from pathlib import Path

from scripts.magik_ci.planning import render, report, selected_paths

ROOT = Path(__file__).resolve().parents[2]


class PlanningTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.root = Path(self.temporary.name).resolve()
        self.git("init", "-q")
        self.git("config", "user.name", "Fixture")
        self.git("config", "user.email", "fixture@example.invalid")
        self.write(".gitignore", "ignored/\n")
        self.write("docs/base.md", "base")
        self.git("add", "--", ".gitignore", "docs/base.md")
        self.git("commit", "-qm", "fixture")

    def tearDown(self):
        self.temporary.cleanup()

    def git(self, *args):
        return (
            subprocess.check_output(["git", "-C", str(self.root), *args])
            .decode()
            .strip()
        )

    def write(self, path, content):
        target = self.root / path
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_text(content)
        return target

    def package(self, directory, features=""):
        self.write(
            directory + "/Cargo.toml",
            '[package]\nname="fixture"\nversion="0.1.0"\n' + features,
        )

    def test_clean_inferred_and_explicit_selection(self):
        self.assertEqual(selected_paths(self.root, None), [])
        self.write("docs/staged.md", "stage")
        self.git("add", "--", "docs/staged.md")
        (self.root / "docs/base.md").unlink()
        self.write("docs/new.md", "new")
        self.write("ignored/cache", "ignore")
        self.assertEqual(
            selected_paths(self.root, None),
            ["docs/base.md", "docs/new.md", "docs/staged.md"],
        )
        self.assertEqual(selected_paths(self.root, []), [])
        self.assertEqual(
            selected_paths(self.root, ["docs/missing.md", "docs/missing.md"]),
            ["docs/missing.md"],
        )
        self.assertEqual(
            selected_paths(self.root, [str(self.root / "docs/base.md")]),
            ["docs/base.md"],
        )

    def test_outside_paths_and_symlinks(self):
        with tempfile.TemporaryDirectory() as outside:
            (self.root / "escape").symlink_to(outside)
            for path in ["../outside.rs", outside + "/new.rs", "escape/new.rs"]:
                with self.subTest(path=path), self.assertRaises(ValueError):
                    selected_paths(self.root, [path])

    def test_mixed_languages_deleted_source_and_unknown_features(self):
        self.package("crates/domain", "[features]\nspecial=[]\n")
        self.write("scripts/tests/test_widget.py", "")
        record = report(
            self.root,
            ["crates/domain/src/deleted.rs", "scripts/widget.py", "docs/base.md"],
        )
        commands = [check["command"] for check in record["local_checks"]]
        self.assertEqual(len(commands), 3)
        self.assertIn(
            [
                "scripts/cargo",
                "test",
                "--manifest-path",
                "crates/domain/Cargo.toml",
                "-p",
                "fixture",
            ],
            commands,
        )
        self.assertIn(
            ["uv", "run", "pytest", "scripts/tests/test_widget.py", "-q"], commands
        )
        self.assertTrue(
            any("additional feature" in item for item in record["unresolved_coverage"])
        )
        self.assertIn("NOT RUN LOCALLY", render(record))
        self.assertEqual(record, json.loads(json.dumps(record)))

    def test_supported_ui_variants_and_missing_feature(self):
        self.package("apps/mister", "[features]\nui=[]\n")
        self.package("apps/desktop", "[features]\ncompiled-ui=[]\n")
        record = report(
            self.root, ["apps/mister/Cargo.toml", "apps/desktop/src/main.rs"]
        )
        commands = [check["command"] for check in record["local_checks"]]
        self.assertEqual(len(commands), 8)
        self.assertTrue(any("ui" in c and "--test-threads=1" in c for c in commands))
        self.assertTrue(any("compiled-ui" in c for c in commands))
        self.package("apps/desktop")
        self.assertTrue(
            any(
                "unavailable" in item
                for item in report(self.root, ["apps/desktop/src/main.rs"])[
                    "unresolved_coverage"
                ]
            )
        )

    def test_submodule_is_not_inspected_and_unknown_coverage_is_reported(self):
        revision = self.git("rev-parse", "HEAD")
        self.git(
            "update-index", "--add", "--cacheinfo", f"160000,{revision},private/nested"
        )
        self.package("private/nested")
        record = report(self.root, ["private/nested/src/lib.rs", "unknown.rs"])
        self.assertEqual(record["local_checks"], [])
        self.assertTrue(
            any(
                "independent submodule" in item
                for item in record["unresolved_coverage"]
            )
        )
        self.assertTrue(
            any("no owning Cargo" in item for item in record["unresolved_coverage"])
        )

    def test_cli_json_and_human_are_stable_and_never_run_validation(self):
        for name in ["cargo", "rustc", "uv", "ssh"]:
            self.write("bin/" + name, "#!/bin/sh\nexit 99\n").chmod(0o755)
        command = [
            "python3",
            str(ROOT / "scripts/checks/pre-push.py"),
            "--repository",
            str(self.root),
            "--plan",
            "--paths",
            "docs/base.md",
        ]
        env = {
            **os.environ,
            "PATH": str(self.root / "bin") + os.pathsep + os.environ["PATH"],
        }
        result = subprocess.check_output(command + ["--json"], env=env, text=True)
        record = json.loads(result)
        self.assertEqual(record["paths"], ["docs/base.md"])
        self.assertEqual(
            subprocess.check_output(command, env=env, text=True).strip(), render(record)
        )


if __name__ == "__main__":
    unittest.main()
