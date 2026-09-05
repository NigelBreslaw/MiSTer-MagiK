# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

"""Exercise pinned LSP startup using temporary local Git repositories."""

import os
import shutil
import subprocess
import tempfile
import unittest
from pathlib import Path

LAUNCHER = Path(__file__).resolve().parents[1] / "rust-lsp"


class RustLspStartupTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.env = {
            **os.environ,
            "GIT_ALLOW_PROTOCOL": "file",
            "GIT_AUTHOR_NAME": "Test",
            "GIT_AUTHOR_EMAIL": "test@example.invalid",
            "GIT_COMMITTER_NAME": "Test",
            "GIT_COMMITTER_EMAIL": "test@example.invalid",
        }
        self.source = self.root / "source"
        self.source.mkdir()
        self.git(self.source, "init", "-q")
        (self.source / "runtime").write_text("old")
        self.git(self.source, "add", ".")
        self.git(self.source, "commit", "-qm", "old")
        self.old = self.git(self.source, "rev-parse", "HEAD").stdout.strip()
        (self.source / "runtime").write_text("pinned")
        self.git(self.source, "commit", "-qam", "pinned")
        self.repo = self.root / "repo"
        self.repo.mkdir()
        self.git(self.repo, "init", "-q")
        self.git(self.repo, "submodule", "add", str(self.source), "private/lspi")
        (self.repo / "scripts").mkdir()
        shutil.copy2(LAUNCHER, self.repo / "scripts/rust-lsp")
        cargo = self.repo / "scripts/cargo"
        cargo.write_text('#!/bin/bash\nprintf "%s\\n" "$@"\n')
        cargo.chmod(0o755)
        self.git(self.repo, "add", ".")
        self.git(self.repo, "commit", "-qm", "fixture")

    def git(self, cwd, *args):
        return subprocess.run(
            ["git", *args],
            cwd=cwd,
            env=self.env,
            text=True,
            capture_output=True,
            check=True,
        )

    def launch(self, repo=None):
        return subprocess.run(
            [str((repo or self.repo) / "scripts/rust-lsp")],
            check=False,
            cwd=self.root,
            env=self.env,
            text=True,
            capture_output=True,
        )

    def test_clean_stale_runtime_is_updated_to_pin(self):
        runtime = self.repo / "private/lspi"
        self.git(runtime, "checkout", "--detach", self.old)
        result = self.launch()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual((runtime / "runtime").read_text(), "pinned")
        self.assertIn(str(self.repo / ".codex/lspi.toml"), result.stdout)
        self.assertNotIn("Submodule", result.stdout)

    def test_staged_dependency_change_is_preserved(self):
        runtime = self.repo / "private/lspi"
        self.git(runtime, "checkout", "--detach", self.old)
        self.git(self.repo, "add", "private/lspi")
        result = self.launch()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("staged", result.stderr)
        self.assertEqual(
            self.git(runtime, "rev-parse", "HEAD").stdout.strip(), self.old
        )

    def test_local_edits_are_preserved(self):
        runtime_file = self.repo / "private/lspi/runtime"
        runtime_file.write_text("user edit")
        result = self.launch()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("local changes", result.stderr)
        self.assertEqual(runtime_file.read_text(), "user edit")
        self.assertEqual(result.stdout, "")

    def test_linked_worktree_initializes_its_own_runtime(self):
        linked = self.root / "linked"
        self.git(self.repo, "worktree", "add", "--detach", str(linked))
        result = self.launch(linked)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual((linked / "private/lspi/runtime").read_text(), "pinned")
        self.assertIn(str(linked / "private/lspi/Cargo.toml"), result.stdout)
        self.assertIn(str(linked / ".codex/lspi.toml"), result.stdout)


if __name__ == "__main__":
    unittest.main()
