#!/usr/bin/env python3
# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

"""Tests for the durable platform bundle archive."""

from __future__ import annotations

import hashlib
import importlib.util
import json
import shutil
import subprocess
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
        self.protocol = "a" * 64
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
                    f"latch_protocol_sha256={self.protocol}",
                    "latch_protocol_version=2",
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
        checked = (
            self.scanout / "mister_magik_scanout_slots.ko",
            self.scanout / "modinfo.txt",
            self.scanout / "provenance.txt",
            self.scanout / "imports.txt",
        )
        (self.scanout / "SHA256SUMS").write_text(
            "".join(f"{sha(path)}  {path.name}\n" for path in checked)
        )
        bundle.write_component_cache("main", self.main, self.main_id, "100", self.main_revision)
        bundle.write_component_cache("fpga", self.fpga, self.fpga_id, "123", "8" * 40)
        bundle.write_component_cache("kernel", self.scanout, self.kernel_id, "456", "9" * 40)

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
        self.assertEqual(payload["latch_protocol_sha256"], self.protocol)
        self.assertEqual(payload["latch_protocol_version"], 2)
        self.assertEqual(payload["latch_rbf_sha256"], sha(self.fpga / "patched/menu-magik-vblank-latch.rbf"))
        self.assertEqual(payload["release_version"], 2)
        self.assertEqual(archive.name, "mister-magik-platform-v0.2.zip")

    def test_protocol_or_rbf_identity_tampering_is_rejected(self) -> None:
        archive = self.create()
        manifest = self.root / "output/platform-bundle-v0.2.json"
        payload = json.loads(manifest.read_text())
        payload["latch_protocol_version"] = 3
        manifest.write_text(json.dumps(payload))
        with self.assertRaisesRegex(ValueError, "release manifest differs|protocol version"):
            bundle.verify(archive, manifest)

    def test_legacy_fpga_metadata_may_have_protocol_hash_without_version(self) -> None:
        for flavour in ("stock", "patched"):
            metadata = self.fpga / flavour / "menu-magik-vblank-latch.metadata.txt"
            metadata.write_text(metadata.read_text().replace("latch_protocol_version=2\n", ""))

        self.assertEqual(
            bundle.verify_fpga_component(self.fpga, self.fpga_id, require_protocol=False),
            self.contract,
        )
        with self.assertRaisesRegex(ValueError, "protocol"):
            bundle.verify_fpga_component(self.fpga, self.fpga_id, require_protocol=True)

    def test_attended_crt_evidence_requires_exact_identity_and_every_gate(self) -> None:
        evidence = self.root / "crt-evidence.json"
        payload = {
            "format": "mister-magik-crt-qualification-v2",
            "qualified": True,
            "identity": {
                "app_revision": "1" * 40,
                "main_revision": self.main_revision,
                "rbf_sha256": "2" * 64,
                "platform_contract_sha256": self.contract,
                "latch_protocol_sha256": self.protocol,
                "latch_protocol_version": 2,
                "platform_manifest_sha256": "3" * 64,
            },
            "trial": {
                "duration_ms": 30_100,
                "mode": "crt-240p60",
                "frames": 1800,
                "flips": 1800,
                "presentation_failures": 0,
            },
            "measurements": {
                "pixel_clock_hz": 12_587_000,
                "horizontal_hz": 15_734.2,
                "vertical_hz": 60.055,
                "h_active": 640,
                "h_front_porch": 30,
                "h_sync_width": 60,
                "h_back_porch": 70,
                "v_active": 240,
                "v_front_porch": 4,
                "v_sync_width": 4,
                "v_back_porch": 14,
                "h_sync_polarity": "negative",
                "v_sync_polarity": "negative",
            },
            "checks": {name: True for name in (
                "launcher_rendering", "core_handoff_native_timing", "osd_and_input",
                "game_launch_and_return", "crash_recovery", "hdmi_resolution_matrix",
                "cleanup_verified", "rollback_verified",
            )},
            "trial_log_sha256": "4" * 64,
            "analyzer": "Morph 4K analog bridge capture",
            "limitations": "Analyzer evidence is not yet an attended real-CRT qualification.",
        }
        evidence.write_text(json.dumps(payload))
        verifier = bundle.ROOT / "scripts/checks/verify-crt-qualification-evidence.py"
        self.assertEqual(subprocess.run([str(verifier), str(evidence)]).returncode, 0)
        payload["checks"]["rollback_verified"] = False
        evidence.write_text(json.dumps(payload))
        self.assertNotEqual(subprocess.run([str(verifier), str(evidence)]).returncode, 0)

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

    def test_verifies_each_standalone_component(self) -> None:
        self.assertEqual(bundle.verify_component("main", self.main, self.main_id, self.main_revision)["component_id"], self.main_id)
        self.assertEqual(bundle.verify_component("fpga", self.fpga, self.fpga_id)["component_id"], self.fpga_id)
        self.assertEqual(bundle.verify_component("kernel", self.scanout, self.kernel_id)["component_id"], self.kernel_id)

    def test_component_cache_matches_upload_artifact_hidden_file_policy(self) -> None:
        hidden = self.fpga / "patched" / "Menu-work" / ".git" / "config"
        hidden.parent.mkdir(parents=True)
        hidden.write_text("not uploaded\n")
        bundle.write_component_cache("fpga", self.fpga, self.fpga_id, "123", "8" * 40)
        checksums = (self.fpga / bundle.COMPONENT_CHECKSUMS).read_text()
        self.assertNotIn(".git/config", checksums)
        hidden.unlink()
        self.assertEqual(bundle.verify_component("fpga", self.fpga, self.fpga_id)["component_id"], self.fpga_id)

    def test_component_cache_accepts_legacy_checksums_for_omitted_hidden_files(self) -> None:
        checksums = self.fpga / bundle.COMPONENT_CHECKSUMS
        checksums.write_text(checksums.read_text() + f"{'0' * 64}  patched/Menu-work/.git/config\n")
        self.assertEqual(bundle.verify_component("fpga", self.fpga, self.fpga_id)["component_id"], self.fpga_id)

    def test_standalone_component_checksum_tampering_is_rejected(self) -> None:
        (self.scanout / "modinfo.txt").write_text("tampered\n")
        with self.assertRaisesRegex(ValueError, "checksum"):
            bundle.verify_component("kernel", self.scanout, self.kernel_id)

    def test_cache_of_cache_preserves_original_build_origin(self) -> None:
        carried_once = self.root / "carried-once"
        carried_twice = self.root / "carried-twice"
        shutil.copytree(self.fpga, carried_once)
        first = bundle.verify_component("fpga", carried_once, self.fpga_id)
        shutil.copytree(carried_once, carried_twice)
        second = bundle.verify_component("fpga", carried_twice, self.fpga_id)
        self.assertEqual(first["origin"], second["origin"])
        self.assertEqual(second["origin"]["run_id"], "123")
        self.assertEqual(second["origin"]["head_sha"], "8" * 40)

    def test_tampered_cache_origin_is_rejected(self) -> None:
        origin = self.fpga / bundle.COMPONENT_ORIGIN
        payload = json.loads(origin.read_text())
        payload["run_id"] = "999"
        origin.write_text(json.dumps(payload, indent=2, sort_keys=True) + "\n")
        with self.assertRaisesRegex(ValueError, "cache checksum"):
            bundle.verify_component("fpga", self.fpga, self.fpga_id)

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
            "main_source": "built-in-current-run", "fpga_source": "reused-from-latest-release", "kernel_source": "reused-from-actions-cache",
            "release_version": 3, "output": self.root / "unified-output",
        })()
        payload = bundle.verify(bundle.create(args))
        self.assertEqual(payload["components"]["main"]["source"], "built-in-current-run")
        self.assertEqual(payload["components"]["fpga"]["source"], "reused-from-latest-release")
        self.assertEqual(payload["components"]["kernel"]["source"], "reused-from-actions-cache")

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
