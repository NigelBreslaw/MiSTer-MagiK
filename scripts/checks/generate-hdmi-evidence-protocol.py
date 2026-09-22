#!/usr/bin/env python3
# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

"""Validate the HDMI evidence schema used by the SystemVerilog generator."""

from __future__ import annotations

import argparse
import json
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
SPEC_PATH = ROOT / "mister/platform/fpga/menu-vblank-latch/hdmi-evidence-protocol.json"


def validate_raw_scaler_record(name: str, record: dict) -> None:
    if len(record["words"]) != record["word_count"] or record["words"][-1] != "crc":
        raise SystemExit(f"{name} layout must match word count and end in CRC")
    used_mask = 0
    for flag_name, bit in record.get("flags", {}).items():
        if bit < 0 or bit > 15 or used_mask & (1 << bit):
            raise SystemExit(f"{name} flag {flag_name} is invalid or overlaps")
        used_mask |= 1 << bit
    for word_name, mask in record.get("reserved_zero_masks", {}).items():
        if word_name not in record["words"] or mask < 0 or mask > 0xFFFF:
            raise SystemExit(f"{name} reserved mask {word_name} is invalid")
        if word_name == "flags" and mask & used_mask:
            raise SystemExit(f"{name} flag and reserved masks overlap")
        for field_name, field in record.get("fields", {}).items():
            if field["word"] == word_name:
                field_mask = ((1 << field["width"]) - 1) << field["bit"]
                if field_mask & mask:
                    raise SystemExit(
                        f"{name} field {field_name} overlaps reserved-zero bits"
                    )
    for field_name, field in record.get("fields", {}).items():
        if field["word"] not in record["words"]:
            raise SystemExit(f"{name} field {field_name} names an unknown word")
        if (
            field["bit"] < 0
            or field["width"] <= 0
            or field["bit"] + field["width"] > 16
        ):
            raise SystemExit(f"{name} field {field_name} is outside one word")


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--check", action="store_true")
    parser.parse_args()
    spec = json.loads(SPEC_PATH.read_text())
    if len(spec["words"]) != spec["word_count"] or spec["words"][-1] != "crc":
        raise SystemExit("HDMI evidence layout must match word count and end in CRC")
    activity = spec["output_activity"]
    if (
        len(activity["words"]) != activity["word_count"]
        or activity["words"][-1] != "crc"
    ):
        raise SystemExit(
            "HDMI output activity layout must match word count and end in CRC"
        )
    path_activity = spec["path_activity"]
    commands = {spec["command"], activity["command"]}
    magics = {spec["magic"], activity["magic"]}
    for name, record in path_activity["records"].items():
        if len(record["words"]) != record["word_count"] or record["words"][-1] != "crc":
            raise SystemExit(f"HDMI {name} layout must match word count and end in CRC")
        if record["command"] in commands or record["magic"] in magics:
            raise SystemExit(f"HDMI {name} command or magic overlaps another record")
        commands.add(record["command"])
        magics.add(record["magic"])
        for counter, offset in record["counters"].items():
            if offset < 0 or offset + path_activity["counter_bits"] > 32:
                raise SystemExit(
                    f"HDMI {name} counter {counter} is outside two packed words"
                )
        for field_name, field in record.get("fields", {}).items():
            if field["word"] not in record["words"]:
                raise SystemExit(
                    f"HDMI {name} field {field_name} names an unknown word"
                )
            if (
                field["bit"] < 0
                or field["width"] <= 0
                or field["bit"] + field["width"] > 16
            ):
                raise SystemExit(f"HDMI {name} field {field_name} is outside one word")
        for word_name, mask in record.get("reserved_zero_masks", {}).items():
            if word_name not in record["words"]:
                raise SystemExit(
                    f"HDMI {name} reserved-zero mask names an unknown word"
                )
            if mask < 0 or mask > 0xFFFF:
                raise SystemExit(f"HDMI {name} reserved-zero mask is outside one word")
            for field_name, field in record.get("fields", {}).items():
                if field["word"] == word_name:
                    field_mask = ((1 << field["width"]) - 1) << field["bit"]
                    if field_mask & mask:
                        raise SystemExit(
                            f"HDMI {name} field {field_name} overlaps reserved-zero bits"
                        )
    raw_scaler = spec["raw_scaler_state"]
    validate_raw_scaler_record("raw scaler state", raw_scaler)
    if raw_scaler["command"] in commands or raw_scaler["magic"] in magics:
        raise SystemExit("raw scaler state command or magic overlaps another record")
    for name, record in spec.get("raw_scaler_rollback_states", {}).items():
        validate_raw_scaler_record(f"raw scaler rollback state {name}", record)
        if (
            record["command"] != raw_scaler["command"]
            or record["magic"] != raw_scaler["magic"]
        ):
            raise SystemExit(
                f"raw scaler rollback state {name} changed command or magic"
            )
    liveness = spec["scaler_fetch_liveness_state"]
    validate_raw_scaler_record("scaler fetch liveness state", liveness)
    if liveness["command"] in commands or liveness["magic"] in magics:
        raise SystemExit(
            "scaler fetch liveness command or magic overlaps another record"
        )
    if (
        liveness["command"] == raw_scaler["command"]
        or liveness["magic"] == raw_scaler["magic"]
    ):
        raise SystemExit(
            "scaler fetch liveness overlaps the fixed raw-scaler transport"
        )


if __name__ == "__main__":
    main()
