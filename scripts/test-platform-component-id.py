#!/usr/bin/env python3
"""Tests for content-addressed platform component identities."""

from __future__ import annotations

import importlib.util
import os
import subprocess
import tempfile
import unittest
from pathlib import Path

SCRIPT = Path(__file__).with_name("platform-component-id.py")
SPEC = importlib.util.spec_from_file_location("platform_component_id", SCRIPT)
assert SPEC and SPEC.loader
component_id = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(component_id)


def git_env() -> dict[str, str]:
    env = os.environ.copy()
    for name in (
        "GIT_DIR",
        "GIT_WORK_TREE",
        "GIT_INDEX_FILE",
        "GIT_OBJECT_DIRECTORY",
        "GIT_ALTERNATE_OBJECT_DIRECTORIES",
        "GIT_COMMON_DIR",
    ):
        env.pop(name, None)
    return env


def run_git(root: Path, *args: str) -> None:
    subprocess.run(["git", "-C", str(root), *args], check=True, env=git_env())


class ComponentIdentityTests(unittest.TestCase):
    def setUp(self) -> None:
        self.temp = tempfile.TemporaryDirectory(prefix="mister-magik-component-id-")
        self.root = Path(self.temp.name)
        for component, inputs in component_id.COMPONENT_INPUTS.items():
            for relative in inputs:
                path = self.root / relative
                if path.suffix or path.name.startswith("."):
                    path.parent.mkdir(parents=True, exist_ok=True)
                    path.write_text(f"{component}:{relative}\n")
                else:
                    path.mkdir(parents=True, exist_ok=True)
                    (path / "input.txt").write_text(f"{component}:{relative}\n")
        subprocess.run(["git", "init", "-q", str(self.root)], check=True, env=git_env())
        run_git(self.root, "config", "user.email", "test@example.invalid")
        run_git(self.root, "config", "user.name", "Test")
        run_git(self.root, "add", ".")
        run_git(self.root, "commit", "-qm", "initial")

    def tearDown(self) -> None:
        self.temp.cleanup()

    def commit(self, message: str) -> None:
        run_git(self.root, "add", ".")
        run_git(self.root, "commit", "-qm", message)

    def test_irrelevant_file_does_not_change_identity(self) -> None:
        before, _ = component_id.component_id(self.root, "fpga")
        (self.root / "docs").mkdir()
        (self.root / "docs/notes.md").write_text("unrelated\n")
        self.commit("docs")
        after, _ = component_id.component_id(self.root, "fpga")
        self.assertEqual(before, after)

    def test_relevant_change_invalidates_identity(self) -> None:
        before, _ = component_id.component_id(self.root, "kernel")
        path = self.root / "kernel/scanout-slots/input.txt"
        path.write_text("changed\n")
        self.commit("kernel")
        after, _ = component_id.component_id(self.root, "kernel")
        self.assertNotEqual(before, after)

    def test_generated_untracked_file_does_not_change_identity(self) -> None:
        before, _ = component_id.component_id(self.root, "kernel")
        (self.root / "kernel/scanout-slots/mister_magik_scanout_slots.ko").write_bytes(b"generated")
        after, _ = component_id.component_id(self.root, "kernel")
        self.assertEqual(before, after)

    def test_dirty_checkout_is_rejected(self) -> None:
        (self.root / "scripts/build-fpga-vblank-latch-core.sh").write_text("dirty\n")
        with self.assertRaisesRegex(ValueError, "clean checkout"):
            component_id.component_id(self.root, "fpga")

    def test_bundle_identity_is_ordered_and_validated(self) -> None:
        fpga = "a" * 64
        kernel = "b" * 64
        self.assertNotEqual(component_id.bundle_id(fpga, kernel), component_id.bundle_id(kernel, fpga))
        with self.assertRaisesRegex(ValueError, "invalid fpga"):
            component_id.bundle_id("bad", kernel)


if __name__ == "__main__":
    unittest.main()
