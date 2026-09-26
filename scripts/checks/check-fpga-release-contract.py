#!/usr/bin/env python3
# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

"""Check that release consumers accept the architecture emitted by the FPGA builder."""

from __future__ import annotations

import importlib.util
import json
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT))
bundle = importlib.import_module("scripts.magik_ci.bundle")


def check() -> str:
    protocol = json.loads(
        (
            ROOT / "mister/platform/fpga/menu-vblank-latch/hdmi-evidence-protocol.json"
        ).read_text()
    )
    architecture = protocol["causal_boundary_state"]["architecture"]
    spec = importlib.util.spec_from_file_location(
        "fpga_release_manifest", ROOT / "scripts/checks/verify-fpga-rbf-manifest.py"
    )
    if spec is None or spec.loader is None:
        raise RuntimeError("cannot load FPGA manifest verifier")
    verifier = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(verifier)
    if architecture not in verifier.DIAGNOSTIC_ARCHITECTURES:
        raise ValueError(
            f"FPGA manifest verifier rejects active architecture: {architecture}"
        )
    bundle._validate_diagnostic_architecture(
        architecture, architecture, historical_baseline=False
    )
    return architecture


if __name__ == "__main__":
    print(f"FPGA release contract verified: {check()}")
