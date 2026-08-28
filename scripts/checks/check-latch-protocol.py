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
HDMI_EVIDENCE_SPEC_PATH = (
    ROOT / "mister/platform/fpga/menu-vblank-latch/hdmi-evidence-protocol.json"
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
        str(ROOT / "scripts/checks/generate-hdmi-evidence-protocol.py"),
        "--check",
    ],
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
if video_diagnostics["schema"] != 4:
    raise SystemExit("video diagnostics protocol schema must be v4")
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
hdmi_evidence = json.loads(HDMI_EVIDENCE_SPEC_PATH.read_text())
if hdmi_evidence["schema"] != 1:
    raise SystemExit("HDMI evidence protocol schema must be v1")
if hdmi_evidence["command"] in set(spec["commands"].values()) | set(
    video_diagnostics["commands"].values()
):
    raise SystemExit("HDMI evidence command overlaps an existing platform command")
if hdmi_evidence["magic"] in set(video_diagnostics["magic"].values()):
    raise SystemExit("HDMI evidence magic overlaps an existing diagnostics record")
if hdmi_evidence["magic"] != 0x4D50:
    raise SystemExit("HDMI evidence magic must be exactly 0x4d50")
if hdmi_evidence["command"] != 0x60:
    raise SystemExit("HDMI evidence command must be exactly 0x60")
if hdmi_evidence["word_count"] != 4:
    raise SystemExit("HDMI lock evidence v1 must contain exactly four words")
if hdmi_evidence["words"] != ["schema", "flags", "lock_loss_count", "crc"]:
    raise SystemExit("HDMI lock evidence v1 word layout changed without a schema update")
hdmi_flag_bits = list(hdmi_evidence["flags"].values())
if len(hdmi_flag_bits) != len(set(hdmi_flag_bits)) or any(
    bit < 0 or bit > 15 for bit in hdmi_flag_bits
):
    raise SystemExit("HDMI evidence flag bits must be unique u16 positions")
if hdmi_evidence["flags"] != {
    "lock_seen_high": 0,
    "lock_armed": 1,
    "lock_current": 2,
    "lock_ever_lost": 3,
    "lock_loss_count_overflow": 4,
}:
    raise SystemExit("HDMI lock evidence v1 flags changed without a schema update")
if hdmi_evidence["crc"] != {
    "polynomial": 0x1021,
    "initial": 0xFFFF,
    "final_xor": 0,
    "word_byte_order": "high-low",
    "header_words": ["command", "schema", "non_crc_word_count"],
}:
    raise SystemExit("HDMI lock evidence CRC parameters changed unexpectedly")
output_activity = hdmi_evidence["output_activity"]
if output_activity != {
    "schema": 1,
    "command": 0x61,
    "magic": 0x4D51,
    "word_count": 6,
    "counter_bits": 4,
    "flags": {
        "frame_valid": 0,
        "counter_collision": 1,
    },
    "words": [
        "schema",
        "flags",
        "no_de_count",
        "de_all_zero_count",
        "de_has_nonzero_count",
        "crc",
    ],
}:
    raise SystemExit("HDMI output activity v1 changed without a schema update")
if output_activity["command"] in (
    set(spec["commands"].values())
    | set(video_diagnostics["commands"].values())
    | {hdmi_evidence["command"]}
):
    raise SystemExit("HDMI output activity command overlaps an existing platform command")
if output_activity["magic"] in set(video_diagnostics["magic"].values()) | {
    hdmi_evidence["magic"]
}:
    raise SystemExit("HDMI output activity magic overlaps an existing diagnostics record")
path_activity = hdmi_evidence["path_activity"]
if path_activity != {
    "counter_bits": 4,
    "records": {
        "final_path": {
            "schema": 1,
            "command": 0x62,
            "magic": 0x4D52,
            "word_count": 5,
            "flags": {"frame_valid": 0, "counter_collision": 1},
            "words": ["schema", "flags", "black_counts", "activity_counts", "crc"],
            "counters": {
                "black_direct": 0,
                "black_scaled": 4,
                "black_mixed": 8,
                "de_has_nonzero": 12,
                "no_de": 16,
            },
        },
        "scaler_raw": {
            "schema": 1,
            "command": 0x63,
            "magic": 0x4D53,
            "word_count": 4,
            "flags": {"frame_valid": 0, "counter_collision": 1},
            "words": ["schema", "flags", "counts", "crc"],
            "counters": {"no_de": 0, "de_all_zero": 4, "de_has_nonzero": 8},
        },
        "post_osd": {
            "schema": 1,
            "command": 0x64,
            "magic": 0x4D54,
            "word_count": 4,
            "flags": {"frame_valid": 0, "counter_collision": 1},
            "words": ["schema", "flags", "counts", "crc"],
            "counters": {"no_de": 0, "de_all_zero": 4, "de_has_nonzero": 8},
        },
        "avalon_liveness": {
            "schema": 1,
            "command": 0x65,
            "magic": 0x4D55,
            "word_count": 4,
            "flags": {"bucket_valid": 0},
            "words": ["schema", "flags", "counts", "crc"],
            "counters": {"request": 0, "accepted": 4, "returned": 8, "bucket": 12},
        },
        "scaler_fetch": {
            "schema": 1,
            "command": 0x66,
            "magic": 0x4D56,
            "word_count": 5,
            "flags": {
                "snapshot_valid": 0,
                "completion_delta_invalid": 1,
                "completion_level_invalid": 2,
            },
            "words": ["schema", "reserved_state", "events", "flags", "crc"],
            "counters": {},
            "reserved_zero_masks": {
                "reserved_state": 0xffff,
                "events": 0xff0c,
            },
            "fields": {
                "batch_two_count": {"word": "events", "bit": 0, "width": 2},
                "starved_frame_count": {"word": "events", "bit": 4, "width": 4},
            },
        },
    },
}:
    raise SystemExit("HDMI path activity v1 records changed without a schema update")
platform_commands = (
    set(spec["commands"].values())
    | set(video_diagnostics["commands"].values())
    | {hdmi_evidence["command"], output_activity["command"]}
)
platform_magics = set(video_diagnostics["magic"].values()) | {
    hdmi_evidence["magic"],
    output_activity["magic"],
}
for name, record in path_activity["records"].items():
    if record["command"] in platform_commands:
        raise SystemExit(f"HDMI {name} command overlaps an existing platform command")
    if record["magic"] in platform_magics:
        raise SystemExit(f"HDMI {name} magic overlaps an existing diagnostics record")
    platform_commands.add(record["command"])
    platform_magics.add(record["magic"])
raw_scaler = hdmi_evidence["raw_scaler_state"]
if raw_scaler != {
    "schema": 11,
    "command": 0x67,
    "magic": 0x4D57,
    "word_count": 5,
    "words": [
        "schema",
        "flags",
        "capture_sequence",
        "ordered_signature",
        "crc",
    ],
    "flags": {
        "capture_valid": 0,
        "fifo_overflow": 1,
        "unexpected_return": 2,
        "bad_burstcount": 3,
        "bad_return_phase": 4,
        "epoch_overlap": 5,
        "counter_overflow": 6,
    },
    "reserved_zero_masks": {"flags": 0xFF80},
}:
    raise SystemExit("scaler-fetch ordered-signature schema 11 changed without an ABI update")
if hdmi_evidence.get("raw_scaler_rollback_states") != {
    "ordered_signature_v3": {
        "schema": 10,
        "command": 0x67,
        "magic": 0x4D57,
        "word_count": 5,
        "words": [
            "schema",
            "flags",
            "frame_sequence",
            "ordered_signature",
            "crc",
        ],
        "flags": {"frame_valid": 0},
        "reserved_zero_masks": {"flags": 0xFFFE},
    }
}:
    raise SystemExit("raw scaler ordered-signature schema-10 rollback ABI changed")
scaler_fetch_liveness = hdmi_evidence.get("scaler_fetch_liveness_state")
if scaler_fetch_liveness != {
    "schema": 14,
    "architecture": "scaler-fetch-liveness-first-stall-v1",
    "command": 0x68,
    "magic": 0x4D58,
    "word_count": 6,
    "words": [
        "schema",
        "flags",
        "sequence_identity",
        "live_state",
        "frozen_state",
        "crc",
    ],
    "flags": {
        "record_valid": 0,
        "normal_liveness_seen": 1,
        "first_stall_valid": 2,
        "observer_fault": 3,
        "reset_ambiguity": 4,
        "reset_level": 5,
        "reset_seen": 6,
        "bad_burstcount": 7,
        "unexpected_return": 8,
        "fifo_phase_error": 9,
        "request_cancelled": 10,
        "counter_ambiguous": 11,
    },
    "reserved_zero_masks": {
        "flags": 0xF000,
        "sequence_identity": 0xFF00,
        "live_state": 0x8000,
    },
    "fields": {
        "publication_sequence": {"word": "sequence_identity", "bit": 0, "width": 8},
        "return_phase": {"word": "live_state", "bit": 0, "width": 7},
        "fifo_depth": {"word": "live_state", "bit": 7, "width": 2},
        "monitor_state": {"word": "live_state", "bit": 9, "width": 2},
        "scoreboard_armed": {"word": "live_state", "bit": 11, "width": 1},
        "first_return_seen": {"word": "live_state", "bit": 12, "width": 1},
        "reset_qualified": {"word": "live_state", "bit": 13, "width": 1},
        "address_wrap_seen": {"word": "live_state", "bit": 14, "width": 1},
        "frozen_cause": {"word": "frozen_state", "bit": 0, "width": 3},
        "frozen_return_phase": {"word": "frozen_state", "bit": 3, "width": 7},
        "frozen_fifo_depth": {"word": "frozen_state", "bit": 10, "width": 2},
        "frozen_address_fold": {"word": "frozen_state", "bit": 12, "width": 4},
    },
    "causes": {
        "none": 0,
        "no_request_seen": 1,
        "accept_blocked": 2,
        "first_return_missing": 3,
        "return_incomplete": 4,
        "request_cancelled": 5,
        "observer_fault": 6,
        "reserved": 7,
    },
    "monitor_states": {
        "unqualified": 0,
        "no_request": 1,
        "accept_blocked": 2,
        "return_progress": 3,
    },
    "constants": {
        "required_burstcount": 128,
        "return_beats": 128,
        "reset_qualify_cycles": 4,
        "watchdog_cycles": 0xFFFFFF,
    },
}:
    raise SystemExit("scaler-fetch liveness schema 14 changed without an ABI update")
if raw_scaler["command"] in platform_commands:
    raise SystemExit("raw scaler ordered-frame command overlaps an existing platform command")
if raw_scaler["magic"] in platform_magics:
    raise SystemExit("raw scaler ordered-frame magic overlaps an existing diagnostics record")
platform_commands.add(raw_scaler["command"])
platform_magics.add(raw_scaler["magic"])
if scaler_fetch_liveness["command"] in platform_commands:
    raise SystemExit("scaler-fetch liveness command overlaps an existing platform command")
if scaler_fetch_liveness["magic"] in platform_magics:
    raise SystemExit("scaler-fetch liveness magic overlaps an existing diagnostics record")
print("latch protocol contract: ok")
