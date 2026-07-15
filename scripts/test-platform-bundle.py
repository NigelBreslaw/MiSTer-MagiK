#!/usr/bin/env python3
# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

"""Tests for the durable platform bundle archive."""

from __future__ import annotations

import hashlib
import importlib.util
import json
import tempfile
import unittest
import zipfile
from pathlib import Path

SCRIPT = Path(__file__).with_name("platform-bundle.py")
SPEC = importlib.util.spec_from_file_location("platform_bundle", SCRIPT)
assert SPEC and SPEC.loader
bundle = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(bundle)


def sha(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


class PlatformBundleTests(unittest.TestCase):
    def setUp(self) -> None:
        self.temp = tempfile.TemporaryDirectory(prefix="mister-magik-platform-bundle-")
        self.root = Path(self.temp.name)
        self.fpga = self.root / "fpga"
        self.scanout = self.root / "scanout"
        self.fpga_id = "a" * 64
        self.kernel_id = "b" * 64
        self.contract = "c" * 64
        for flavour in ("stock", "patched"):
            directory = self.fpga / flavour
            reports = directory / "reports"
            reports.mkdir(parents=True)
            rbf = directory / "menu-magik-vblank-latch.rbf"
            report = reports / "quartus-delta-signoff.tsv"
            rbf.write_bytes(f"{flavour} rbf".encode())
            report.write_bytes(b"signoff\n")
            (directory / "menu-magik-vblank-latch.metadata.txt").write_text(
                "\n".join((
                    "format=mister-magik-fpga-release-v1",
                    f"platform_contract_sha256={self.contract}",
                    "magik_commit=" + "1" * 40,
                    "builder_commit=" + "2" * 40,
                    "source_commit=" + "3" * 40,
                    "patch_sha256=" + "4" * 64,
                    "latch_rtl_sha256=" + "5" * 64,
                    f"component_input_sha256={self.fpga_id}",
                    "component_revision=" + "6" * 40,
                    "quartus_seed=1",
                    "quartus_version=17.0.0 Build 595",
                    "workflow_url=https://github.example/actions/runs/1",
                    "signoff_valid=1",
                    "build_date=260713",
                    "rbf_file=menu-magik-vblank-latch.rbf",
                    f"rbf_sha256={sha(rbf)}",
                    f"report_sha256.reports/quartus-delta-signoff.tsv={sha(report)}",
                )) + "\n"
            )
        self.scanout.mkdir()
        module = self.scanout / "mister_magik_scanout_slots.ko"
        module.write_bytes(b"module")
        (self.scanout / "provenance.txt").write_text(
            f"platform_contract_sha256={self.contract}\n"
            f"component_input_sha256={self.kernel_id}\n"
            f"component_revision={'7' * 40}\n"
            f"module_sha256={sha(module)}\n"
        )
        (self.scanout / "modinfo.txt").write_text("vermagic: 5.15.1-MiSTer\n")
        (self.scanout / "imports.txt").write_text("")
        (self.scanout / "SHA256SUMS").write_text("")

    def tearDown(self) -> None:
        self.temp.cleanup()

    def create(self) -> Path:
        args = type("Args", (), {
            "fpga_dir": self.fpga,
            "scanout_dir": self.scanout,
            "fpga_id": self.fpga_id,
            "kernel_id": self.kernel_id,
            "fpga_run_id": "123",
            "kernel_run_id": "456",
            "fpga_head_sha": "8" * 40,
            "kernel_head_sha": "9" * 40,
            "release_version": 2,
            "output": self.root / "output",
        })()
        return bundle.create(args)

    def test_round_trip(self) -> None:
        archive = self.create()
        payload = bundle.verify(archive, self.root / "output/platform-bundle-v0.1.json")
        self.assertEqual(payload["fpga_input_sha256"], self.fpga_id)
        self.assertEqual(payload["kernel_input_sha256"], self.kernel_id)
        self.assertEqual(payload["release_version"], 2)
        self.assertEqual(archive.name, "mister-magik-platform-v0.2.zip")

    def test_update_plan_starts_at_one(self) -> None:
        plan = bundle.update_plan(None, 0, self.fpga_id, self.kernel_id)
        self.assertEqual(plan["next_version"], 1)
        self.assertEqual(plan["release_tag"], "platform-v0.1")
        self.assertTrue(plan["update_needed"])

    def test_update_plan_increments_only_for_changed_identity(self) -> None:
        archive = self.create()
        current = bundle.verify(archive)
        unchanged = bundle.update_plan(current, 2, self.fpga_id, self.kernel_id)
        changed = bundle.update_plan(current, 2, self.fpga_id, "d" * 64)
        self.assertFalse(unchanged["update_needed"])
        self.assertEqual(unchanged["next_version"], 3)
        self.assertTrue(changed["update_needed"])
        self.assertEqual(changed["next_version"], 3)
        self.assertEqual(changed["release_tag"], "platform-v0.3")

    def test_update_plan_accepts_legacy_v1_manifest(self) -> None:
        current = bundle.verify(self.create())
        current.pop("release_version")
        plan = bundle.update_plan(current, 1, self.fpga_id, self.kernel_id)
        self.assertFalse(plan["update_needed"])

    def test_update_plan_rejects_tag_manifest_version_mismatch(self) -> None:
        current = bundle.verify(self.create())
        with self.assertRaisesRegex(ValueError, "tag and manifest"):
            bundle.update_plan(current, 3, self.fpga_id, self.kernel_id)

    def test_verify_rejects_tag_manifest_version_mismatch(self) -> None:
        with self.assertRaisesRegex(ValueError, "tag and manifest"):
            bundle.verify(self.create(), expected_release_version=3)

    def test_verify_accepts_legacy_v1_without_release_version(self) -> None:
        archive = self.create()
        legacy = self.root / "legacy.zip"
        with zipfile.ZipFile(archive) as source, zipfile.ZipFile(legacy, "w") as target:
            manifest = json.loads(source.read("platform-bundle-v0.1.json"))
            manifest.pop("release_version")
            for info in source.infolist():
                payload = source.read(info.filename)
                if info.filename == "platform-bundle-v0.1.json":
                    payload = (json.dumps(manifest, indent=2, sort_keys=True) + "\n").encode()
                elif info.filename == "SHA256SUMS":
                    lines = [
                        line
                        for line in payload.decode().splitlines()
                        if not line.endswith("  platform-bundle-v0.1.json")
                    ]
                    manifest_bytes = (
                        json.dumps(manifest, indent=2, sort_keys=True) + "\n"
                    ).encode()
                    manifest_hash = hashlib.sha256(manifest_bytes).hexdigest()
                    payload = (
                        "\n".join(lines + [f"{manifest_hash}  platform-bundle-v0.1.json"])
                        + "\n"
                    ).encode()
                target.writestr(info, payload)
        payload = bundle.verify(legacy, expected_release_version=1)
        self.assertNotIn("release_version", payload)

    def test_tampering_is_rejected(self) -> None:
        archive = self.create()
        altered = self.root / "altered.zip"
        with zipfile.ZipFile(archive) as source, zipfile.ZipFile(altered, "w") as target:
            for info in source.infolist():
                payload = source.read(info.filename)
                if info.filename == "scanout/mister_magik_scanout_slots.ko":
                    payload = b"tampered"
                target.writestr(info, payload)
        with self.assertRaisesRegex(ValueError, "manifest|checksum|provenance"):
            bundle.verify(altered)

    def test_mixed_contract_is_rejected(self) -> None:
        metadata = self.fpga / "patched/menu-magik-vblank-latch.metadata.txt"
        metadata.write_text(metadata.read_text().replace(self.contract, "d" * 64))
        with self.assertRaisesRegex(ValueError, "mixed"):
            self.create()

    def test_archive_traversal_is_rejected(self) -> None:
        archive = self.root / "unsafe.zip"
        with zipfile.ZipFile(archive, "w") as output:
            output.writestr("../unsafe", "no")
        with self.assertRaisesRegex(ValueError, "unsafe"):
            bundle.verify(archive)


if __name__ == "__main__":
    unittest.main()
