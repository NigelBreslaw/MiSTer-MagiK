#!/usr/bin/env python3
# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

"""Validate the canonical latch protocol and its generated consumers."""

from __future__ import annotations

import json
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
SPEC_PATH = ROOT / "mister/platform/fpga/menu-vblank-latch/latch-protocol.json"


def crc16_ccitt_false(words: list[int]) -> int:
    crc = 0xFFFF
    for word in words:
        for byte in ((word >> 8) & 0xFF, word & 0xFF):
            crc ^= byte << 8
            for _ in range(8):
                crc = ((crc << 1) ^ 0x1021) & 0xFFFF if crc & 0x8000 else (crc << 1) & 0xFFFF
    return crc


subprocess.run(
    [sys.executable, str(ROOT / "scripts/checks/generate-latch-protocol.py"), "--check"],
    check=True,
)
spec = json.loads(SPEC_PATH.read_text())
if spec["active_protocol_version"] not in (2, 3):
    raise SystemExit("active latch protocol must be exactly v2 or v3")
if spec["protocols"]["2"] != {
    "flags": 7,
    "caps_words": 5,
    "set_payload_words": 11,
    "status_payload_words": 11,
    "caps_crc": False,
    "set_crc": False,
    "status_crc": False,
}:
    raise SystemExit("protocol-v2 wire profile changed")
if len(spec["set_words_v3"]) != 12 or spec["set_words_v3"][-1] != "crc":
    raise SystemExit("protocol-v3 SET must contain eleven payload words plus CRC")
if len(spec["status_words_v3"]) != 14 or spec["status_words_v3"][-1] != "crc":
    raise SystemExit("protocol-v3 status must contain thirteen payload words plus CRC")
if spec["protocols"]["3"]["flags"] != 0x007F:
    raise SystemExit("protocol-v3 capability flags must be the exact complete profile")
for name, golden in spec["goldens"].items():
    words = [
        golden["command"],
        3,
        len(golden["payload"]),
        *golden["payload"],
    ]
    actual = crc16_ccitt_false(words)
    if actual != golden["crc"]:
        raise SystemExit(
            f"{name} CRC mismatch: expected 0x{golden['crc']:04x}, got 0x{actual:04x}"
        )
print("latch protocol contract: ok")
