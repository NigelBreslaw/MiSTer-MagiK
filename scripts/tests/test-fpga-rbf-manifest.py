#!/usr/bin/env python3
# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

"""Fixture tests for the FPGA release manifest verifier."""

from __future__ import annotations

import hashlib
from pathlib import Path
import subprocess
import tempfile
import unittest

SCRIPT = Path(__file__).resolve().parents[1] / "checks/verify-fpga-rbf-manifest.py"


class ManifestTest(unittest.TestCase):
    def fixture(self, root: Path, *, historical_v2: bool = False) -> Path:
        rbf = root / "release.rbf"
        report = root / "reports/fit.rpt"
        delta = root / "reports/quartus-delta-signoff.tsv"
        report.parent.mkdir()
        rbf.write_bytes(b"release-rbf")
        report.write_bytes(b"fit-report")
        delta.write_bytes(b"quartus_delta_signoff_tsv\tvalid=1\n")
        sha = lambda path: hashlib.sha256(path.read_bytes()).hexdigest()
        metadata = root / "release.metadata.txt"
        metadata.write_text(
            "\n".join((
                "format="
                + (
                    "mister-magik-fpga-release-v1"
                    if historical_v2
                    else "mister-magik-fpga-release-v2"
                ),
                "platform_contract_sha256=" + "6" * 64,
                "magik_commit=" + "1" * 40,
                "builder_commit=" + "5" * 40,
                "source_commit=" + "2" * 40,
                "patch_sha256=" + "3" * 64,
                "latch_rtl_sha256=" + "4" * 64,
                *(() if historical_v2 else ("latch_bridge_sha256=" + "8" * 64,)),
                *(
                    ()
                    if historical_v2
                    else (
                        "component_input_sha256=" + "9" * 64,
                        "component_revision=" + "a" * 40,
                    )
                ),
                "latch_protocol_sha256=" + "7" * 64,
                "latch_protocol_version=" + ("2" if historical_v2 else "5"),
                *(() if historical_v2 else ("latch_capability_mask=0x03ff",)),
                "quartus_seed=1",
                "quartus_version=17.0.0 Build 595",
                "workflow_url=https://github.example/actions/runs/1",
                "signoff_valid=1",
                "build_date=260711",
                "rbf_file=release.rbf",
                "rbf_sha256=" + sha(rbf),
                "report_sha256.reports/fit.rpt=" + sha(report),
                "report_sha256.reports/quartus-delta-signoff.tsv=" + sha(delta),
            )) + "\n"
        )
        return metadata

    def run_verify(
        self,
        metadata: Path,
        *,
        historical_v2: bool = False,
    ) -> subprocess.CompletedProcess[str]:
        command = [str(SCRIPT)]
        if historical_v2:
            command.append("--historical-v2")
        command.append(str(metadata))
        return subprocess.run(command, text=True, capture_output=True)

    def test_matching_fixture_passes(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            self.assertEqual(self.run_verify(self.fixture(Path(directory))).returncode, 0)

    def test_modified_rbf_fails(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            metadata = self.fixture(Path(directory))
            (Path(directory) / "release.rbf").write_bytes(b"modified")
            self.assertNotEqual(self.run_verify(metadata).returncode, 0)

    def test_modified_or_missing_metadata_fails(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            metadata = self.fixture(Path(directory))
            metadata.write_text(metadata.read_text().replace("quartus_seed=1", "quartus_seed=2"))
            self.assertNotEqual(self.run_verify(metadata).returncode, 0)
            self.assertNotEqual(self.run_verify(Path(directory) / "missing.txt").returncode, 0)

    def test_controlled_analysis_constraint_override_passes(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            metadata = self.fixture(Path(directory))
            metadata.write_text(
                metadata.read_text()
                + "source_status= M sys/sys_top.sdc\n"
                + "analysis_constraint_override=clock_groups_exclusive_to_asynchronous\n"
            )
            self.assertEqual(self.run_verify(metadata).returncode, 0)

    def test_unproven_or_unexpected_source_change_fails(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            metadata = self.fixture(Path(directory))
            valid = metadata.read_text()
            metadata.write_text(valid + "source_status= M sys/sys_top.sdc\n")
            self.assertNotEqual(self.run_verify(metadata).returncode, 0)
            metadata.write_text(
                valid
                + "source_status= M sys/sys_top.v\n"
                + "analysis_constraint_override=clock_groups_exclusive_to_asynchronous\n"
            )
            self.assertNotEqual(self.run_verify(metadata).returncode, 0)
            metadata.write_text(
                valid
                + "analysis_constraint_override=clock_groups_exclusive_to_asynchronous\n"
            )
            self.assertNotEqual(self.run_verify(metadata).returncode, 0)

    def test_missing_or_invalid_platform_contract_fails(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            metadata = self.fixture(Path(directory))
            valid = metadata.read_text()
            metadata.write_text(valid.replace("platform_contract_sha256=" + "6" * 64 + "\n", ""))
            self.assertNotEqual(self.run_verify(metadata).returncode, 0)
            metadata.write_text(valid.replace("platform_contract_sha256=" + "6" * 64, "platform_contract_sha256=bad"))
            self.assertNotEqual(self.run_verify(metadata).returncode, 0)

    def test_mailbox_era_metadata_fails(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            metadata = self.fixture(Path(directory))
            metadata.write_text(metadata.read_text() + "mailbox_module_sha256=" + "5" * 64 + "\n")
            self.assertNotEqual(self.run_verify(metadata).returncode, 0)

    def test_missing_or_wrong_protocol_identity_fails(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            metadata = self.fixture(Path(directory))
            valid = metadata.read_text()
            metadata.write_text(valid.replace("latch_protocol_version=5\n", ""))
            self.assertNotEqual(self.run_verify(metadata).returncode, 0)
            metadata.write_text(
                valid.replace("latch_protocol_version=5", "latch_protocol_version=3")
            )
            self.assertNotEqual(self.run_verify(metadata).returncode, 0)
            metadata.write_text(
                valid.replace(
                    "latch_capability_mask=0x03ff",
                    "latch_capability_mask=0x01fe",
                )
            )
            self.assertNotEqual(self.run_verify(metadata).returncode, 0)

    def test_historical_v2_requires_explicit_rollback_mode(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            metadata = self.fixture(Path(directory), historical_v2=True)
            self.assertNotEqual(self.run_verify(metadata).returncode, 0)
            self.assertEqual(
                self.run_verify(metadata, historical_v2=True).returncode,
                0,
            )

    def test_new_v4_artifact_is_not_historical_v2(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            metadata = self.fixture(Path(directory))
            self.assertNotEqual(
                self.run_verify(metadata, historical_v2=True).returncode,
                0,
            )


if __name__ == "__main__":
    unittest.main()
