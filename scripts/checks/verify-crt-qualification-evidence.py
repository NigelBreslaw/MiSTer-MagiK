#!/usr/bin/env python3
# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

"""Verify attended CRT evidence tied to one exact qualified platform."""

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
    "crt_hdmi_crt_switching",
    "osd_and_input",
    "game_launch_and_return",
    "crash_recovery",
    "hdmi_resolution_matrix",
    "cleanup_verified",
    "rollback_verified",
}


def verify(path: Path) -> dict[str, object]:
    payload = json.loads(path.read_text())
    if payload.get("format") != "mister-magik-crt-qualification-v1":
        raise ValueError("unsupported CRT qualification format")
    if payload.get("qualified") is not True:
        raise ValueError("hardware evidence must explicitly be qualified")
    identity = payload.get("identity")
    if not isinstance(identity, dict):
        raise ValueError("missing platform identity")
    for name in ("app_revision", "main_revision"):
        if not HEX40.fullmatch(str(identity.get(name, ""))):
            raise ValueError(f"invalid {name}")
    for name in (
        "rbf_sha256",
        "platform_contract_sha256",
        "latch_protocol_sha256",
        "platform_manifest_sha256",
    ):
        if not HEX64.fullmatch(str(identity.get(name, ""))):
            raise ValueError(f"invalid {name}")
    if identity.get("latch_protocol_version") != 3:
        raise ValueError("qualification must use latch protocol version 3")

    trial = payload.get("trial")
    if not isinstance(trial, dict):
        raise ValueError("missing trial measurements")
    duration = int(trial.get("duration_ms", 0))
    horizontal_hz = float(trial.get("horizontal_hz", 0))
    vertical_hz = float(trial.get("vertical_hz", 0))
    if not 30_000 <= duration <= 35_000:
        raise ValueError("CRT trial duration is outside the attended bound")
    if not 15_700 <= horizontal_hz <= 15_770:
        raise ValueError("horizontal timing is outside the 240p60 qualification range")
    if not 59.9 <= vertical_hz <= 60.2:
        raise ValueError("vertical timing is outside the 240p60 qualification range")
    if trial.get("underruns") != 0 or trial.get("timeouts") != 0:
        raise ValueError("CRT trial contains scanout errors")
    if trial.get("fallback") is not False:
        raise ValueError("CRT trial entered fallback")

    checks = payload.get("checks")
    if not isinstance(checks, dict) or set(checks) != REQUIRED_CHECKS:
        raise ValueError("CRT qualification check set is incomplete")
    failed = sorted(name for name, passed in checks.items() if passed is not True)
    if failed:
        raise ValueError("failed attended checks: " + ", ".join(failed))
    if not HEX64.fullmatch(str(payload.get("trial_log_sha256", ""))):
        raise ValueError("invalid trial_log_sha256")
    if not isinstance(payload.get("limitations"), str):
        raise ValueError("limitations must be recorded")
    return payload


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("evidence", type=Path)
    args = parser.parse_args()
    try:
        payload = verify(args.evidence)
    except (OSError, ValueError, TypeError, json.JSONDecodeError) as error:
        print(f"CRT qualification invalid: {error}", file=sys.stderr)
        return 1
    print(f"crt_qualification_valid=1 app_revision={payload['identity']['app_revision']}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

