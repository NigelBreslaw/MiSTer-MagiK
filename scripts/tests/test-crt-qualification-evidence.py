#!/usr/bin/env python3
# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

"""Fixture tests for protocol-versioned CRT qualification evidence."""

from __future__ import annotations

import json
from pathlib import Path
import subprocess
import tempfile
import unittest

SCRIPT = Path(__file__).resolve().parents[1] / "checks/verify-crt-qualification-evidence.py"


class CrtEvidenceTest(unittest.TestCase):
    def fixture(self, root: Path, *, historical_v2: bool = False) -> Path:
        identity: dict[str, object] = {
            "app_revision": "1" * 40,
            "main_revision": "2" * 40,
            "rbf_sha256": "3" * 64,
            "platform_contract_sha256": "4" * 64,
            "latch_protocol_sha256": "5" * 64,
            "platform_manifest_sha256": "6" * 64,
            "latch_protocol_version": 2 if historical_v2 else 5,
        }
        if not historical_v2:
            identity.update(
                {
                    "menu_revision": "7" * 40,
                    "kernel_sha256": "8" * 64,
                    "fpga_component_id": "9" * 64,
                    "candidate_workflow_url": "https://github.example/actions/runs/1",
                    "latch_capability_mask": "0x03ff",
                }
            )
        payload = {
            "format": (
                "mister-magik-crt-qualification-v2"
                if historical_v2
                else "mister-magik-crt-qualification-v3"
            ),
            "qualified": True,
            "identity": identity,
            "trial": {
                "mode": "crt-240p60",
                "duration_ms": 30_000,
                "frames": 1_800,
                "flips": 1_800,
                "presentation_failures": 0,
            },
            "measurements": {
                "pixel_clock_hz": 12_587_000,
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
                "horizontal_hz": 15_733.75,
                "vertical_hz": 60.052,
            },
            "checks": {
                "launcher_rendering": True,
                "core_handoff_native_timing": True,
                "osd_and_input": True,
                "game_launch_and_return": True,
                "crash_recovery": True,
                "hdmi_resolution_matrix": True,
                "cleanup_verified": True,
                "rollback_verified": True,
            },
            "trial_log_sha256": "a" * 64,
            "analyzer": "fixture analyzer",
            "limitations": "fixture only",
        }
        evidence = root / "crt-evidence.json"
        evidence.write_text(json.dumps(payload))
        return evidence

    def run_verify(
        self,
        evidence: Path,
        *,
        historical_v2: bool = False,
    ) -> subprocess.CompletedProcess[str]:
        command = [str(SCRIPT)]
        if historical_v2:
            command.append("--historical-v2")
        command.append(str(evidence))
        return subprocess.run(command, text=True, capture_output=True)

    def test_new_evidence_requires_complete_v4_identity(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            evidence = self.fixture(Path(directory))
            self.assertEqual(self.run_verify(evidence).returncode, 0)
            payload = json.loads(evidence.read_text())
            del payload["identity"]["fpga_component_id"]
            evidence.write_text(json.dumps(payload))
            self.assertNotEqual(self.run_verify(evidence).returncode, 0)
            payload["identity"]["fpga_component_id"] = "9" * 64
            payload["identity"]["latch_capability_mask"] = "0x01fe"
            evidence.write_text(json.dumps(payload))
            self.assertNotEqual(self.run_verify(evidence).returncode, 0)

    def test_v2_is_explicitly_historical_only(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            evidence = self.fixture(Path(directory), historical_v2=True)
            self.assertNotEqual(self.run_verify(evidence).returncode, 0)
            self.assertEqual(
                self.run_verify(evidence, historical_v2=True).returncode,
                0,
            )

    def test_v4_cannot_use_historical_mode(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            evidence = self.fixture(Path(directory))
            self.assertNotEqual(
                self.run_verify(evidence, historical_v2=True).returncode,
                0,
            )


if __name__ == "__main__":
    unittest.main()
