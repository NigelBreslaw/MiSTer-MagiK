#!/usr/bin/env python3
# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

"""Tests for content-addressed platform component identities."""

from __future__ import annotations

import importlib.util
import os
import subprocess
import tempfile
import unittest
from pathlib import Path

SCRIPT = (
    Path(__file__).resolve().parents[1] / "release/platform/platform-component-id.py"
)
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
        source_root = SCRIPT.parents[3]
        for manifest_relative in component_id.COMPONENT_INPUT_MANIFESTS.values():
            source = source_root / manifest_relative
            destination = self.root / manifest_relative
            destination.parent.mkdir(parents=True, exist_ok=True)
            destination.write_text(source.read_text())
        implementation = self.root / component_id.IDENTITY_IMPLEMENTATION
        implementation.parent.mkdir(parents=True, exist_ok=True)
        implementation.write_text("identity implementation\n")
        for component in component_id.COMPONENT_INPUT_MANIFESTS:
            for relative in component_id.component_inputs(self.root, component):
                if relative in (
                    *component_id.COMPONENT_INPUT_MANIFESTS.values(),
                    component_id.IDENTITY_IMPLEMENTATION,
                ):
                    continue
                path = self.root / relative
                if path.suffix or path.name.startswith("."):
                    path.parent.mkdir(parents=True, exist_ok=True)
                    if not path.exists():
                        path.write_text(f"{component}:{relative}\n")
                else:
                    path.mkdir(parents=True, exist_ok=True)
                    fixture = path / "input.txt"
                    if not fixture.exists():
                        fixture.write_text(f"{component}:{relative}\n")
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
        fpga_before, _ = component_id.component_id(self.root, "fpga")
        kernel_before, _ = component_id.component_id(self.root, "kernel")
        (self.root / "docs").mkdir()
        (self.root / "docs/notes.md").write_text("unrelated\n")
        self.commit("docs")
        fpga_after, _ = component_id.component_id(self.root, "fpga")
        kernel_after, _ = component_id.component_id(self.root, "kernel")
        self.assertEqual(fpga_before, fpga_after)
        self.assertEqual(kernel_before, kernel_after)

    def test_kernel_input_change_only_invalidates_kernel_identity(self) -> None:
        fpga_before, _ = component_id.component_id(self.root, "fpga")
        kernel_before, _ = component_id.component_id(self.root, "kernel")
        path = self.root / "mister/platform/kernel/scanout-slots/input.txt"
        path.write_text("changed\n")
        self.commit("kernel")
        fpga_after, _ = component_id.component_id(self.root, "fpga")
        kernel_after, _ = component_id.component_id(self.root, "kernel")
        self.assertEqual(fpga_before, fpga_after)
        self.assertNotEqual(kernel_before, kernel_after)

    def test_fpga_manifest_change_leaves_kernel_identity_unchanged(self) -> None:
        fpga_before, _ = component_id.component_id(self.root, "fpga")
        synthesis_before, _ = component_id.component_id(self.root, "fpga-synthesis")
        kernel_before, _ = component_id.component_id(self.root, "kernel")
        manifest = self.root / component_id.COMPONENT_INPUT_MANIFESTS["fpga"]
        manifest.write_text(
            manifest.read_text() + "mister/platform/fpga/extra-identity-input.txt\n"
        )
        extra = self.root / "mister/platform/fpga/extra-identity-input.txt"
        extra.write_text("extra FPGA input\n")
        self.commit("extend FPGA identity manifest")
        fpga_after, _ = component_id.component_id(self.root, "fpga")
        synthesis_after, _ = component_id.component_id(self.root, "fpga-synthesis")
        kernel_after, _ = component_id.component_id(self.root, "kernel")
        self.assertNotEqual(fpga_before, fpga_after)
        self.assertEqual(synthesis_before, synthesis_after)
        self.assertEqual(kernel_before, kernel_after)

    def test_validation_change_does_not_invalidate_fpga_synthesis(self) -> None:
        fpga_before, _ = component_id.component_id(self.root, "fpga")
        synthesis_before, _ = component_id.component_id(self.root, "fpga-synthesis")
        validator = self.root / "scripts/checks/check-fpga-quartus-delta.py"
        validator.write_text("fixed validation\n")
        self.commit("fix FPGA validation")
        fpga_after, _ = component_id.component_id(self.root, "fpga")
        synthesis_after, _ = component_id.component_id(self.root, "fpga-synthesis")
        self.assertNotEqual(fpga_before, fpga_after)
        self.assertEqual(synthesis_before, synthesis_after)

    def test_synthesis_change_invalidates_fpga_component_and_synthesis(self) -> None:
        fpga_before, _ = component_id.component_id(self.root, "fpga")
        synthesis_before, _ = component_id.component_id(self.root, "fpga-synthesis")
        build = self.root / "scripts/build-fpga-vblank-latch-core.sh"
        build.write_text("changed synthesis\n")
        self.commit("change FPGA synthesis")
        fpga_after, _ = component_id.component_id(self.root, "fpga")
        synthesis_after, _ = component_id.component_id(self.root, "fpga-synthesis")
        self.assertNotEqual(fpga_before, fpga_after)
        self.assertNotEqual(synthesis_before, synthesis_after)

    def test_kernel_manifest_change_only_invalidates_kernel_identity(self) -> None:
        fpga_before, _ = component_id.component_id(self.root, "fpga")
        kernel_before, _ = component_id.component_id(self.root, "kernel")
        manifest = self.root / component_id.COMPONENT_INPUT_MANIFESTS["kernel"]
        manifest.write_text(
            manifest.read_text() + "mister/platform/kernel/extra-identity-input.txt\n"
        )
        extra = self.root / "mister/platform/kernel/extra-identity-input.txt"
        extra.write_text("extra kernel input\n")
        self.commit("extend kernel identity manifest")
        fpga_after, _ = component_id.component_id(self.root, "fpga")
        kernel_after, _ = component_id.component_id(self.root, "kernel")
        self.assertEqual(fpga_before, fpga_after)
        self.assertNotEqual(kernel_before, kernel_after)

    def test_shared_identity_implementation_invalidates_both_components(self) -> None:
        fpga_before, _ = component_id.component_id(self.root, "fpga")
        kernel_before, _ = component_id.component_id(self.root, "kernel")
        implementation = self.root / component_id.IDENTITY_IMPLEMENTATION
        implementation.write_text("changed identity implementation\n")
        self.commit("change shared identity implementation")
        fpga_after, _ = component_id.component_id(self.root, "fpga")
        kernel_after, _ = component_id.component_id(self.root, "kernel")
        self.assertNotEqual(fpga_before, fpga_after)
        self.assertNotEqual(kernel_before, kernel_after)

    def test_canonical_revision_includes_identity_definitions(self) -> None:
        before = component_id.component_revision(self.root, "fpga")
        implementation = self.root / component_id.IDENTITY_IMPLEMENTATION
        implementation.write_text("revision-defining identity implementation\n")
        self.commit("change canonical identity implementation")
        after = component_id.component_revision(self.root, "fpga")
        self.assertNotEqual(before, after)
        self.assertEqual(
            after,
            subprocess.run(
                ["git", "-C", str(self.root), "rev-parse", "HEAD"],
                check=True,
                text=True,
                capture_output=True,
                env=git_env(),
            ).stdout.strip(),
        )

    def test_generated_untracked_file_does_not_change_identity(self) -> None:
        fpga_before, _ = component_id.component_id(self.root, "fpga")
        kernel_before, _ = component_id.component_id(self.root, "kernel")
        (
            self.root
            / "mister/platform/kernel/scanout-slots/mister_magik_scanout_slots.ko"
        ).write_bytes(b"generated")
        fpga_after, _ = component_id.component_id(self.root, "fpga")
        kernel_after, _ = component_id.component_id(self.root, "kernel")
        self.assertEqual(fpga_before, fpga_after)
        self.assertEqual(kernel_before, kernel_after)

    def test_dirty_checkout_is_rejected(self) -> None:
        (self.root / "scripts/build-fpga-vblank-latch-core.sh").write_text("dirty\n")
        with self.assertRaisesRegex(ValueError, "clean checkout"):
            component_id.component_id(self.root, "fpga")

    def test_bundle_identity_is_ordered_and_validated(self) -> None:
        fpga = "a" * 64
        kernel = "b" * 64
        self.assertNotEqual(
            component_id.bundle_id(fpga, kernel), component_id.bundle_id(kernel, fpga)
        )
        with self.assertRaisesRegex(ValueError, "invalid fpga"):
            component_id.bundle_id("bad", kernel)


if __name__ == "__main__":
    unittest.main()
