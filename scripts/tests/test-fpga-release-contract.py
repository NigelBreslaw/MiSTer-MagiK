#!/usr/bin/env python3
# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

"""Offline regression checks for release-policy drift before synthesis."""

import importlib.util
import json
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[2]
spec = importlib.util.spec_from_file_location(
    "release_contract", ROOT / "scripts/checks/check-fpga-release-contract.py"
)
assert spec is not None and spec.loader is not None
contract = importlib.util.module_from_spec(spec)
spec.loader.exec_module(contract)


class ReleaseContractTest(unittest.TestCase):
    def test_active_contract_passes_both_release_consumers(self):
        contract.check()

    def test_stale_bundle_policy_rejects_active_contract(self):
        with patch.object(
            contract.bundle,
            "PATCHED_DIAGNOSTIC_ARCHITECTURE",
            "scaler-off-domain-scheduler-terminal-v6",
        ):
            with self.assertRaisesRegex(ValueError, "fpga_diagnostic_architecture"):
                contract.check()

    def test_stale_manifest_policy_rejects_active_contract(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            protocol = (
                root
                / "mister/platform/fpga/menu-vblank-latch/hdmi-evidence-protocol.json"
            )
            protocol.parent.mkdir(parents=True)
            protocol.write_text(
                json.dumps({"causal_boundary_state": {"architecture": "future-v1"}})
            )
            verifier = root / "scripts/checks/verify-fpga-rbf-manifest.py"
            verifier.parent.mkdir(parents=True)
            verifier.write_text("DIAGNOSTIC_ARCHITECTURES = {'old-v1'}\n")
            with patch.object(contract, "ROOT", root):
                with self.assertRaisesRegex(
                    ValueError,
                    "manifest verifier rejects active architecture: future-v1",
                ):
                    contract.check()


if __name__ == "__main__":
    unittest.main()
