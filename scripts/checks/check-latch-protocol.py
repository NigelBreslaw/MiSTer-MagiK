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
VIDEO_DIAGNOSTICS_SPEC_PATH = (
    ROOT / "mister/platform/fpga/menu-vblank-latch/video-diagnostics-protocol.json"
)


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
subprocess.run(
    [
        sys.executable,
        str(ROOT / "scripts/checks/generate-video-diagnostics-protocol.py"),
        "--check",
    ],
    check=True,
)
spec = json.loads(SPEC_PATH.read_text())
if spec["schema"] != 5:
    raise SystemExit("latch protocol schema must be v5")
if spec["active_protocol_version"] != 5 or set(spec["protocols"]) != {"5"}:
    raise SystemExit("only latch protocol v5 may be generated")
if len(spec["set_words_v5"]) != 12 or spec["set_words_v5"][-1] != "crc":
    raise SystemExit("protocol-v5 SET must contain eleven payload words plus CRC")
if len(spec["status_words_v5"]) != 16 or spec["status_words_v5"][-1] != "crc":
    raise SystemExit("protocol-v5 status must contain fifteen payload words plus CRC")
if len(spec["receipt_words_v5"]) != 11 or spec["receipt_words_v5"][-1] != "crc":
    raise SystemExit("protocol-v5 receipt must contain ten payload words plus CRC")
if (
    len(spec["presentation_telemetry_words_v5"]) != 11
    or spec["presentation_telemetry_words_v5"][-1] != "crc"
):
    raise SystemExit("protocol-v5 presentation telemetry must contain ten payload words plus CRC")
if spec["protocols"]["5"]["flags"] != 0x03FF:
    raise SystemExit("protocol-v5 capability flags must be exactly 0x03ff")
if spec["capabilities"].get("authoritative_presentation_telemetry") != 9:
    raise SystemExit("protocol-v5 presentation telemetry capability must be bit 9")
for name, golden in spec["goldens"].items():
    words = [
        golden["command"],
        5,
        len(golden["payload"]),
        *golden["payload"],
    ]
    actual = crc16_ccitt_false(words)
    if actual != golden["crc"]:
        raise SystemExit(
            f"{name} CRC mismatch: expected 0x{golden['crc']:04x}, got 0x{actual:04x}"
        )

video_diagnostics = json.loads(VIDEO_DIAGNOSTICS_SPEC_PATH.read_text())
if video_diagnostics["schema"] != 1:
    raise SystemExit("video diagnostics protocol schema must be v1")
if set(video_diagnostics["commands"].values()) & set(spec["commands"].values()):
    raise SystemExit("video diagnostics commands overlap the latch protocol")
if set(video_diagnostics["commands"].values()) != {0x5D, 0x5E, 0x5F}:
    raise SystemExit("video diagnostics commands must be exactly 0x5d-0x5f")
for group, word_count in video_diagnostics["word_counts"].items():
    words = video_diagnostics[f"{group}_words"]
    if len(words) != word_count or words[-1] != "crc":
        raise SystemExit(f"video diagnostics {group} layout must match its fixed CRC word count")
if len(set(video_diagnostics["magic"].values())) != len(video_diagnostics["magic"]):
    raise SystemExit("video diagnostics magic values must be unique")
for group, flags in video_diagnostics["flags"].items():
    bits = list(flags.values())
    if len(bits) != len(set(bits)) or any(bit < 0 or bit > 15 for bit in bits):
        raise SystemExit(f"video diagnostics {group} flag bits must be unique u16 positions")
print("latch protocol contract: ok")
