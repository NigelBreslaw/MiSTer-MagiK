#!/usr/bin/env python3
# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

"""Verify attended analyzer evidence for the shared MiSTer Direct Video path."""

from __future__ import annotations

import argparse
import json
import re
import sys
from pathlib import Path

HEX40 = re.compile(r"^[0-9a-f]{40}$")
HEX64 = re.compile(r"^[0-9a-f]{64}$")
REQUIRED_CHECKS = {
    "launcher_rendering",
    "core_handoff_native_timing",
    "osd_and_input",
    "game_launch_and_return",
    "crash_recovery",
    "hdmi_resolution_matrix",
    "cleanup_verified",
    "rollback_verified",
}
MODE_TIMINGS = {
    "crt-240p60": (12_587_000, 640, 30, 60, 70, 240, 4, 4, 14),
    "crt-288p50": (12_587_000, 640, 30, 60, 70, 288, 6, 4, 14),
    "crt-480p60": (25_175_000, 640, 16, 96, 48, 480, 8, 4, 33),
    "crt-576p50": (25_175_000, 640, 16, 96, 48, 576, 2, 4, 42),
}


def close(actual: float, expected: float, tolerance: float) -> bool:
    return abs(actual - expected) <= expected * tolerance


def verify(path: Path, *, historical_v2: bool = False) -> dict[str, object]:
    payload = json.loads(path.read_text())
    expected_format = (
        "mister-magik-crt-qualification-v2"
        if historical_v2
        else "mister-magik-crt-qualification-v3"
    )
    if payload.get("format") != expected_format:
        raise ValueError(f"CRT qualification must use {expected_format}")
    if payload.get("qualified") is not True:
        raise ValueError("hardware evidence must explicitly be qualified")
    identity = payload.get("identity")
    if not isinstance(identity, dict):
        raise ValueError("missing platform identity")
    revision_names = ["app_revision", "main_revision"]
    if not historical_v2:
        revision_names.append("menu_revision")
    for name in revision_names:
        if not HEX40.fullmatch(str(identity.get(name, ""))):
            raise ValueError(f"invalid {name}")
    hash_names = [
        "rbf_sha256",
        "platform_contract_sha256",
        "latch_protocol_sha256",
        "platform_manifest_sha256",
    ]
    if not historical_v2:
        hash_names.extend(("kernel_sha256", "fpga_component_id"))
    for name in hash_names:
        if not HEX64.fullmatch(str(identity.get(name, ""))):
            raise ValueError(f"invalid {name}")
    expected_protocol = 2 if historical_v2 else 5
    if identity.get("latch_protocol_version") != expected_protocol:
        raise ValueError(
            f"qualification must use latch protocol version {expected_protocol}"
        )
    if not historical_v2 and identity.get("latch_capability_mask") != "0x03ff":
        raise ValueError("qualification must use latch capability mask 0x03ff")
    if not historical_v2 and not str(identity.get("candidate_workflow_url", "")).startswith(
        "https://"
    ):
        raise ValueError("candidate_workflow_url must identify immutable CI evidence")

    trial = payload.get("trial")
    if not isinstance(trial, dict):
        raise ValueError("missing bounded publication trial")
    mode = trial.get("mode")
    expected = MODE_TIMINGS.get(mode)
    if expected is None:
        raise ValueError("trial mode is not a standard progressive Direct Video mode")
    if not 30_000 <= int(trial.get("duration_ms", 0)) <= 35_000:
        raise ValueError("CRT trial duration is outside the attended bound")
    if int(trial.get("frames", 0)) <= 0 or int(trial.get("flips", 0)) <= 0:
        raise ValueError("CRT trial did not advance shared latch publication")
    if trial.get("presentation_failures") != 0:
        raise ValueError("CRT trial contains presentation failures")

    measurements = payload.get("measurements")
    if not isinstance(measurements, dict):
        raise ValueError("missing external analyzer measurements")
    names = (
        "pixel_clock_hz",
        "h_active",
        "h_front_porch",
        "h_sync_width",
        "h_back_porch",
        "v_active",
        "v_front_porch",
        "v_sync_width",
        "v_back_porch",
    )
    for name, value in zip(names, expected):
        actual = float(measurements.get(name, 0))
        if name == "pixel_clock_hz":
            if not close(actual, value, 0.01):
                raise ValueError("pixel clock is outside the Main mode tolerance")
        elif int(actual) != value:
            raise ValueError(f"{name} does not match Main's standard mode")
    if measurements.get("h_sync_polarity") != "negative":
        raise ValueError("horizontal sync polarity must be recorded as negative")
    if measurements.get("v_sync_polarity") != "negative":
        raise ValueError("vertical sync polarity must be recorded as negative")
    h_total = sum(int(measurements[name]) for name in names[1:5])
    v_total = sum(int(measurements[name]) for name in names[5:])
    pixel_clock = float(measurements["pixel_clock_hz"])
    if not close(float(measurements.get("horizontal_hz", 0)), pixel_clock / h_total, 0.01):
        raise ValueError("measured horizontal rate is inconsistent with clock and totals")
    if not close(float(measurements.get("vertical_hz", 0)), pixel_clock / h_total / v_total, 0.01):
        raise ValueError("measured vertical rate is inconsistent with clock and totals")

    checks = payload.get("checks")
    if not isinstance(checks, dict) or set(checks) != REQUIRED_CHECKS:
        raise ValueError("CRT qualification check set is incomplete")
    failed = sorted(name for name, passed in checks.items() if passed is not True)
    if failed:
        raise ValueError("failed attended checks: " + ", ".join(failed))
    if not HEX64.fullmatch(str(payload.get("trial_log_sha256", ""))):
        raise ValueError("invalid trial_log_sha256")
    if not isinstance(payload.get("analyzer"), str) or not payload["analyzer"].strip():
        raise ValueError("external analyzer must be identified")
    if not isinstance(payload.get("limitations"), str):
        raise ValueError("limitations must be recorded")
    return payload


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("evidence", type=Path)
    parser.add_argument(
        "--historical-v2",
        action="store_true",
        help="verify retained protocol-v2 evidence; never use for a new qualification",
    )
    args = parser.parse_args()
    try:
        payload = verify(args.evidence, historical_v2=args.historical_v2)
    except (OSError, ValueError, TypeError, json.JSONDecodeError) as error:
        print(f"CRT qualification invalid: {error}", file=sys.stderr)
        return 1
    print(f"crt_qualification_valid=1 app_revision={payload['identity']['app_revision']}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
