#!/usr/bin/env python3
# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

"""Regression tests for the complete production platform manifest."""

from __future__ import annotations

import hashlib
import importlib.util
import tempfile
import unittest
from pathlib import Path

SCRIPT = Path(__file__).resolve().parents[1] / "release/platform/platform-manifest.py"
SPEC = importlib.util.spec_from_file_location("platform_manifest", SCRIPT)
assert SPEC and SPEC.loader
platform_manifest = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(platform_manifest)


def sha(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


class PlatformManifestTests(unittest.TestCase):
    def setUp(self) -> None:
        self.temp = tempfile.TemporaryDirectory(prefix="mister-magik-platform-")
        self.root = Path(self.temp.name)
        self.magik = "2" * 40
        self.main_revision = "1" * 40
        self.menu_revision = "3" * 40
        self.contract = "4" * 64
        for relative in (
            "MiSTer_MagiK",
            "mister-magik/mister-magik-fb",
            "mister-magik/mister_magik_scanout_slots.ko",
            "mister-magik/fpga/menu-magik-vblank-latch.rbf",
        ):
            path = self.root / relative
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(relative.encode())
        self.module = self.root / "mister-magik/mister_magik_scanout_slots.ko"
        self.module_meta = self.root / "mister-magik/mister_magik_scanout_slots.metadata.txt"
        self.rbf = self.root / "mister-magik/fpga/menu-magik-vblank-latch.rbf"
        self.rbf_meta = self.root / "mister-magik/fpga/menu-magik-vblank-latch.metadata.txt"
        self.module_meta.write_text(
            f"platform_contract_sha256={self.contract}\nmodule_sha256={sha(self.module)}\n"
        )
        self.rbf_meta.write_text(
            "format=mister-magik-fpga-release-v1\n"
            f"platform_contract_sha256={self.contract}\n"
            f"magik_commit={self.magik}\nsource_commit={self.menu_revision}\n"
            f"rbf_sha256={sha(self.rbf)}\n"
        )
        self.manifest = self.root / "mister-magik/platform-v2.manifest"
        values = {
            "format": platform_manifest.FORMAT,
            **{
                f"{name}_path": path
                for name, path in platform_manifest.LAYOUT_PATHS["public"].items()
            },
            "main_sha256": sha(self.root / "MiSTer_MagiK"),
            "gui_sha256": sha(self.root / "mister-magik/mister-magik-fb"),
            "scanout_module_sha256": sha(self.module),
            "scanout_metadata_sha256": sha(self.module_meta),
            "latch_rbf_sha256": sha(self.rbf),
            "latch_metadata_sha256": sha(self.rbf_meta),
            "platform_contract_sha256": self.contract,
            "main_revision": self.main_revision,
            "magik_revision": self.magik,
            "menu_revision": self.menu_revision,
        }
        self.valid = "".join(
            f"{field}={values[field]}\n" for field in platform_manifest.FIELDS
        )
        self.manifest.write_text(self.valid)

    def tearDown(self) -> None:
        self.temp.cleanup()

    def verify(self) -> None:
        platform_manifest.verify_manifest(
            self.manifest, artifact_root=self.root, layout="public"
        )

    def test_complete_bundle_is_valid(self) -> None:
        self.verify()

    def test_missing_field_is_rejected(self) -> None:
        self.manifest.write_text(self.valid.replace("main_revision=" + self.main_revision + "\n", ""))
        with self.assertRaisesRegex(ValueError, "missing manifest fields"):
            self.verify()

    def test_malformed_field_is_rejected(self) -> None:
        self.manifest.write_text(self.valid.replace("main_sha256=" + sha(self.root / "MiSTer_MagiK"), "main_sha256=no"))
        with self.assertRaisesRegex(ValueError, "invalid main_sha256"):
            self.verify()

    def test_duplicate_field_is_rejected(self) -> None:
        self.manifest.write_text(self.valid + "format=mister-magik-platform-v2\n")
        with self.assertRaisesRegex(ValueError, "duplicate field format"):
            self.verify()

    def test_mixed_contract_is_rejected(self) -> None:
        self.rbf_meta.write_text(self.rbf_meta.read_text().replace(self.contract, "5" * 64))
        self.manifest.write_text(self.valid.replace(
            "latch_metadata_sha256=" + sha(self.rbf_meta),
            "latch_metadata_sha256=" + sha(self.rbf_meta),
        ))
        with self.assertRaisesRegex(ValueError, "incorrect installed latch_metadata hash|mixed framebuffer"):
            self.verify()

    def test_incorrect_artifact_hash_is_rejected(self) -> None:
        self.rbf.write_bytes(b"corrupt")
        with self.assertRaisesRegex(ValueError, "incorrect installed latch_rbf hash"):
            self.verify()

    def test_catalog_builder_is_not_part_of_manifest(self) -> None:
        self.assertNotIn("catalog_builder", self.valid)

    def test_old_experiments_path_is_rejected(self) -> None:
        self.manifest.write_text(self.valid.replace(
            "/media/fat/mister-magik/fpga/menu-magik-vblank-latch.rbf",
            "/media/fat/mister-magik/" + "experiments/menu-magik-vblank-latch.rbf",
        ))
        with self.assertRaisesRegex(ValueError, "incorrect latch_rbf_path"):
            self.verify()

    def test_root_menu_rbf_is_never_part_of_manifest(self) -> None:
        self.assertNotIn("/media/fat/menu.rbf", self.valid)

    def test_public_manifest_is_rejected_as_dev(self) -> None:
        with self.assertRaisesRegex(ValueError, "incorrect main_path"):
            platform_manifest.verify_manifest(
                self.manifest, artifact_root=self.root, layout="dev"
            )

    def test_dev_paths_are_rejected_as_public(self) -> None:
        dev = self.valid
        for name, public_path in platform_manifest.LAYOUT_PATHS["public"].items():
            dev = dev.replace(
                f"{name}_path={public_path}",
                f"{name}_path={platform_manifest.LAYOUT_PATHS['dev'][name]}",
            )
        self.manifest.write_text(dev)
        with self.assertRaisesRegex(ValueError, "incorrect main_path"):
            self.verify()


if __name__ == "__main__":
    unittest.main()
