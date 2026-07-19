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

SCRIPT = Path(__file__).resolve().parents[1] / "release/platform/platform-bundle.py"
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
        self.main = self.root / "main"
        self.main_revision = "e" * 40
        self.main_id = bundle.load_module(
            "test_main_component", bundle.ROOT / "scripts/release/platform/main-component.py"
        ).component_id(self.main_revision)
        self.fpga_id = "a" * 64
        self.kernel_id = "b" * 64
        self.contract = "c" * 64
        self.main.mkdir()
        main_binary = self.main / "MiSTer_MagiK"
        main_binary.write_bytes(b"main")
        main_receipt = {
            "format": "mister-magik-main-component-v0.1",
            "repository": "NigelBreslaw/Main_MiSTer",
            "branch": "mister-magik",
            "source_revision": self.main_revision,
            "toolchain": "gcc-arm-10.2-2020.11-x86_64-arm-none-linux-gnueabihf",
            "component_id": self.main_id,
            "binary": {"path": "MiSTer_MagiK", "size": main_binary.stat().st_size, "sha256": sha(main_binary)},
        }
        receipt = self.main / "main-component-v0.1.json"
        receipt.write_text(json.dumps(main_receipt, indent=2, sort_keys=True) + "\n")
        (self.main / "SHA256SUMS").write_text(
            f"{sha(main_binary)}  MiSTer_MagiK\n{sha(receipt)}  main-component-v0.1.json\n"
        )
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
            "main_dir": self.main,
            "fpga_dir": self.fpga,
            "scanout_dir": self.scanout,
            "main_id": self.main_id,
            "fpga_id": self.fpga_id,
            "kernel_id": self.kernel_id,
            "main_run_id": "100",
            "fpga_run_id": "123",
            "kernel_run_id": "456",
            "main_head_sha": self.main_revision,
            "fpga_head_sha": "8" * 40,
            "kernel_head_sha": "9" * 40,
            "release_version": 2,
            "output": self.root / "output",
        })()
        return bundle.create(args)

    def legacy_archive(self) -> Path:
        archive = self.create()
        legacy = self.root / "legacy.zip"
        with zipfile.ZipFile(archive) as source, zipfile.ZipFile(legacy, "w") as target:
            manifest = json.loads(source.read(bundle.MANIFEST_NAME))
            manifest["format"] = bundle.FORMAT_V1
            manifest["bundle_id"] = bundle.legacy_bundle_id(self.fpga_id, self.kernel_id)
            manifest.pop("release_version")
            manifest.pop("main_input_sha256")
            manifest["components"].pop("main")
            manifest["files"] = [entry for entry in manifest["files"] if not entry["path"].startswith("main/")]
            payloads = {
                info.filename: source.read(info.filename)
                for info in source.infolist()
                if not info.filename.startswith("main/") and info.filename not in {bundle.MANIFEST_NAME, "SHA256SUMS"}
            }
            payloads[bundle.MANIFEST_V1] = (json.dumps(manifest, indent=2, sort_keys=True) + "\n").encode()
            payloads["SHA256SUMS"] = "".join(
                f"{hashlib.sha256(value).hexdigest()}  {name}\n"
                for name, value in sorted(payloads.items())
            ).encode()
            for name, payload in payloads.items():
                target.writestr(name, payload)
        (self.root / bundle.MANIFEST_V1).write_bytes(zipfile.ZipFile(legacy).read(bundle.MANIFEST_V1))
        return legacy

    def test_round_trip(self) -> None:
        archive = self.create()
        payload = bundle.verify(archive, self.root / "output/platform-bundle-v0.2.json")
        self.assertEqual(payload["main_input_sha256"], self.main_id)
        self.assertEqual(payload["fpga_input_sha256"], self.fpga_id)
        self.assertEqual(payload["kernel_input_sha256"], self.kernel_id)
        self.assertEqual(payload["release_version"], 2)
        self.assertEqual(archive.name, "mister-magik-platform-v0.2.zip")

    def test_update_plan_starts_at_one(self) -> None:
        plan = bundle.update_plan(None, 0, self.main_id, self.fpga_id, self.kernel_id)
        self.assertEqual(plan["next_version"], 1)
        self.assertEqual(plan["release_tag"], "platform-v0.1")
        self.assertTrue(plan["update_needed"])
        self.assertTrue(plan["main_changed"])
        self.assertTrue(plan["fpga_changed"])
        self.assertTrue(plan["kernel_changed"])

    def test_update_plan_increments_only_for_changed_identity(self) -> None:
        archive = self.create()
        current = bundle.verify(archive)
        unchanged = bundle.update_plan(current, 2, self.main_id, self.fpga_id, self.kernel_id)
        changed = bundle.update_plan(current, 2, self.main_id, self.fpga_id, "d" * 64)
        self.assertFalse(unchanged["update_needed"])
        self.assertFalse(unchanged["main_changed"])
        self.assertFalse(unchanged["fpga_changed"])
        self.assertFalse(unchanged["kernel_changed"])
        self.assertEqual(unchanged["next_version"], 3)
        self.assertTrue(changed["update_needed"])
        self.assertEqual(changed["next_version"], 3)
        self.assertEqual(changed["release_tag"], "platform-v0.3")
        self.assertFalse(changed["main_changed"])
        self.assertFalse(changed["fpga_changed"])
        self.assertTrue(changed["kernel_changed"])

    def test_update_plan_reports_each_changed_component(self) -> None:
        current = bundle.verify(self.create())
        main = bundle.update_plan(current, 2, "d" * 64, self.fpga_id, self.kernel_id)
        fpga = bundle.update_plan(current, 2, self.main_id, "d" * 64, self.kernel_id)
        all_changed = bundle.update_plan(current, 2, "d" * 64, "e" * 64, "f" * 64)
        self.assertEqual((main["main_changed"], main["fpga_changed"], main["kernel_changed"]), (True, False, False))
        self.assertEqual((fpga["main_changed"], fpga["fpga_changed"], fpga["kernel_changed"]), (False, True, False))
        self.assertTrue(all_changed["main_changed"] and all_changed["fpga_changed"] and all_changed["kernel_changed"])

    def test_update_plan_migrates_legacy_v1_manifest(self) -> None:
        current = {
            "format": bundle.FORMAT_V1,
            "bundle_id": bundle.legacy_bundle_id(self.fpga_id, self.kernel_id),
            "fpga_input_sha256": self.fpga_id,
            "kernel_input_sha256": self.kernel_id,
        }
        plan = bundle.update_plan(current, 1, self.main_id, self.fpga_id, self.kernel_id)
        self.assertTrue(plan["update_needed"])

    def test_update_plan_rejects_tag_manifest_version_mismatch(self) -> None:
        current = bundle.verify(self.create())
        with self.assertRaisesRegex(ValueError, "tag and manifest"):
            bundle.update_plan(current, 3, self.main_id, self.fpga_id, self.kernel_id)

    def test_verify_rejects_tag_manifest_version_mismatch(self) -> None:
        with self.assertRaisesRegex(ValueError, "tag and manifest"):
            bundle.verify(self.create(), expected_release_version=3)

    def test_verify_accepts_legacy_v1_without_release_version(self) -> None:
        legacy = self.legacy_archive()
        payload = bundle.verify(legacy, expected_release_version=1)
        self.assertNotIn("release_version", payload)

    def test_extracts_each_exact_v2_component_with_origin(self) -> None:
        archive = self.create()
        manifest = self.root / "output" / bundle.MANIFEST_NAME
        for component, identity, marker in (
            ("main", self.main_id, "MiSTer_MagiK"),
            ("fpga", self.fpga_id, "patched/menu-magik-vblank-latch.rbf"),
            ("kernel", self.kernel_id, "mister_magik_scanout_slots.ko"),
        ):
            output = self.root / f"extracted-{component}"
            result = bundle.extract_component(archive, manifest, component, identity, output)
            self.assertTrue((output / marker).is_file())
            self.assertEqual(result["component_id"], identity)
            self.assertTrue(str(result["run_id"]).isdigit())

    def test_extract_rejects_mismatched_identity(self) -> None:
        with self.assertRaisesRegex(ValueError, "identity"):
            bundle.extract_component(
                self.create(), self.root / "output" / bundle.MANIFEST_NAME,
                "kernel", "d" * 64, self.root / "wrong",
            )

    def test_legacy_extracts_fpga_and_kernel_but_not_main(self) -> None:
        archive = self.legacy_archive()
        manifest = self.root / bundle.MANIFEST_V1
        bundle.extract_component(archive, manifest, "fpga", self.fpga_id, self.root / "legacy-fpga")
        bundle.extract_component(archive, manifest, "kernel", self.kernel_id, self.root / "legacy-kernel")
        with self.assertRaisesRegex(ValueError, "does not contain main"):
            bundle.extract_component(archive, manifest, "main", self.main_id, self.root / "legacy-main")

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

    def test_missing_component_is_rejected(self) -> None:
        archive = self.create()
        altered = self.root / "missing-component.zip"
        with zipfile.ZipFile(archive) as source, zipfile.ZipFile(altered, "w") as target:
            for info in source.infolist():
                if info.filename != "scanout/mister_magik_scanout_slots.ko":
                    target.writestr(info, source.read(info.filename))
        with self.assertRaisesRegex(ValueError, "manifest|missing|module"):
            bundle.verify(altered)

    def test_malformed_manifest_is_rejected(self) -> None:
        archive = self.create()
        altered = self.root / "malformed-manifest.zip"
        with zipfile.ZipFile(archive) as source, zipfile.ZipFile(altered, "w") as target:
            for info in source.infolist():
                payload = b"{not-json\n" if info.filename == bundle.MANIFEST_NAME else source.read(info.filename)
                target.writestr(info, payload)
        with self.assertRaises((ValueError, json.JSONDecodeError)):
            bundle.verify(altered)

    def test_main_tampering_is_rejected(self) -> None:
        archive = self.create()
        altered = self.root / "altered-main.zip"
        with zipfile.ZipFile(archive) as source, zipfile.ZipFile(altered, "w") as target:
            for info in source.infolist():
                payload = source.read(info.filename)
                if info.filename == "main/MiSTer_MagiK":
                    payload = b"tampered"
                target.writestr(info, payload)
        with self.assertRaisesRegex(ValueError, "manifest|checksum|Main"):
            bundle.verify(altered)

    def test_invalid_component_origin_is_rejected(self) -> None:
        archive = self.create()
        altered = self.root / "altered-origin.zip"
        with zipfile.ZipFile(archive) as source, zipfile.ZipFile(altered, "w") as target:
            manifest = json.loads(source.read(bundle.MANIFEST_NAME))
            manifest["components"]["main"]["workflow"] = "untrusted.yml"
            for info in source.infolist():
                payload = source.read(info.filename)
                if info.filename == bundle.MANIFEST_NAME:
                    payload = (json.dumps(manifest, indent=2, sort_keys=True) + "\n").encode()
                target.writestr(info, payload)
        with self.assertRaisesRegex(ValueError, "origin workflow"):
            bundle.verify(altered)

    def test_unified_workflow_and_component_source_are_verified(self) -> None:
        args = type("Args", (), {
            "main_dir": self.main, "fpga_dir": self.fpga, "scanout_dir": self.scanout,
            "main_id": self.main_id, "fpga_id": self.fpga_id, "kernel_id": self.kernel_id,
            "main_run_id": "100", "fpga_run_id": "100", "kernel_run_id": "100",
            "main_head_sha": self.main_revision, "fpga_head_sha": "8" * 40, "kernel_head_sha": "9" * 40,
            "main_workflow": "platform-bundle.yml", "fpga_workflow": "platform-bundle.yml", "kernel_workflow": "platform-bundle.yml",
            "main_source": "built-in-current-run", "fpga_source": "reused-from-latest-release", "kernel_source": "reused-from-latest-release",
            "release_version": 3, "output": self.root / "unified-output",
        })()
        payload = bundle.verify(bundle.create(args))
        self.assertEqual(payload["components"]["main"]["source"], "built-in-current-run")
        self.assertEqual(payload["components"]["fpga"]["source"], "reused-from-latest-release")

    def test_non_numeric_component_run_is_rejected(self) -> None:
        archive = self.create()
        altered = self.root / "altered-run.zip"
        with zipfile.ZipFile(archive) as source, zipfile.ZipFile(altered, "w") as target:
            manifest = json.loads(source.read(bundle.MANIFEST_NAME))
            manifest["components"]["kernel"]["run_id"] = "not-a-run"
            for info in source.infolist():
                payload = source.read(info.filename)
                if info.filename == bundle.MANIFEST_NAME:
                    payload = (json.dumps(manifest, indent=2, sort_keys=True) + "\n").encode()
                target.writestr(info, payload)
        with self.assertRaisesRegex(ValueError, "origin run ID"):
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
