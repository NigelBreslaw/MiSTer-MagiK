#!/usr/bin/env python3
# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

"""Fixture tests for the bootstrap-free pre-push gate."""

from __future__ import annotations

import importlib.util
import subprocess
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
GATE = ROOT / "scripts/checks/pre-push.py"
SPEC = importlib.util.spec_from_file_location("pre_push_gate", GATE)
assert SPEC and SPEC.loader
PRE_PUSH = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(PRE_PUSH)


class Repository:
    def __init__(self) -> None:
        self.temporary = tempfile.TemporaryDirectory(prefix="pre-push-fixture-")
        self.root = Path(self.temporary.name)
        self.run("git", "init", "-q")
        self.run("git", "config", "user.name", "Fixture")
        self.run("git", "config", "user.email", "fixture@example.invalid")

    def close(self) -> None:
        self.temporary.cleanup()

    def run(
        self, *args: str, allowed: tuple[int, ...] = (0,)
    ) -> subprocess.CompletedProcess[str]:
        result = subprocess.run(
            args,
            cwd=self.root,
            text=True,
            capture_output=True,
            check=False,
        )
        if result.returncode not in allowed:
            raise AssertionError(f"{' '.join(args)} failed: {result.stderr}")
        return result

    def commit(self, path: str, contents: str) -> str:
        destination = self.root / path
        destination.parent.mkdir(parents=True, exist_ok=True)
        destination.write_text(contents)
        self.run("git", "add", "--", path)
        self.run("git", "commit", "-qm", path)
        return self.run("git", "rev-parse", "HEAD").stdout.strip()

    def update(self, old: str, new: str) -> str:
        return f"refs/heads/main {new} refs/heads/main {old}\n"


class PrePushTests(unittest.TestCase):
    def setUp(self) -> None:
        self.repository = Repository()

    def tearDown(self) -> None:
        self.repository.close()

    def test_existing_branch_selects_exact_committed_paths(self) -> None:
        base = self.repository.commit("base.txt", "base\n")
        head = self.repository.commit("head.txt", "head\n")
        self.assertEqual(
            PRE_PUSH.pushed_paths(
                self.repository.root, "origin", self.repository.update(base, head)
            ),
            ["head.txt"],
        )

    def test_new_branch_selects_complete_tree_without_remote_head(self) -> None:
        self.repository.commit("base.txt", "base\n")
        head = self.repository.commit("head.txt", "head\n")
        update = f"refs/heads/topic {head} refs/heads/topic {PRE_PUSH.ZERO_OID}\n"
        self.assertEqual(
            PRE_PUSH.pushed_paths(self.repository.root, "origin", update),
            ["base.txt", "head.txt"],
        )

    def test_tags_deletions_and_non_head_updates_are_handled(self) -> None:
        base = self.repository.commit("base.txt", "base\n")
        head = self.repository.commit("head.txt", "head\n")
        updates = (
            f"refs/tags/v1 {head} refs/tags/v1 {PRE_PUSH.ZERO_OID}\n"
            f"refs/heads/old {PRE_PUSH.ZERO_OID} refs/heads/old {base}\n"
        )
        self.assertEqual(
            PRE_PUSH.pushed_paths(self.repository.root, "origin", updates), []
        )
        non_head = f"refs/heads/main {base} refs/heads/main {PRE_PUSH.ZERO_OID}\n"
        with self.assertRaisesRegex(PRE_PUSH.PrePushError, "pre_push_non_head"):
            PRE_PUSH.pushed_paths(self.repository.root, "origin", non_head)

    def test_deleted_paths_are_returned_and_not_required_to_exist(self) -> None:
        base = self.repository.commit("removed.txt", "removed\n")
        self.run_git("rm", "removed.txt")
        self.repository.run("git", "commit", "-qm", "remove")
        head = self.repository.run("git", "rev-parse", "HEAD").stdout.strip()
        update = self.repository.update(base, head)
        self.assertEqual(
            PRE_PUSH.pushed_paths(self.repository.root, "origin", update),
            ["removed.txt"],
        )

    def run_git(self, *args: str) -> None:
        self.repository.run("git", *args)


if __name__ == "__main__":
    unittest.main()
