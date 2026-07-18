#!/usr/bin/env python3
# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

import json
import re
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
spec = json.loads((ROOT / "fpga/menu-vblank-latch/latch-protocol.json").read_text())
rust = (ROOT / "latch-contract/src/lib.rs").read_text()
sv = (ROOT / "fpga/menu-vblank-latch/mister_magik_latch_protocol.svh").read_text()

checks = [
    (rust, rf"SET_FBUF_LATCH: u16 = 0x{spec['set_command']:02x}"),
    (rust, rf"GET_FBUF_LATCH: u16 = 0x{spec['get_command']:02x}"),
    (rust, rf"GET_FBUF_LATCH_CAPS: u16 = 0x{spec['caps_command']:02x}"),
    (rust, rf"LATCH_MAGIC: u16 = 0x{spec['latch_magic']:04x}"),
    (rust, rf"STATUS_MAGIC: u16 = 0x{spec['status_magic']:04x}"),
    (rust, rf"CAPS_MAGIC: u16 = 0x{spec['caps_magic']:04x}"),
    (sv, rf"SET_FBUF_LATCH = 8'h{spec['set_command']:02X}"),
    (sv, rf"GET_FBUF_LATCH = 8'h{spec['get_command']:02X}"),
    (sv, rf"GET_FBUF_LATCH_CAPS = 8'h{spec['caps_command']:02X}"),
    (sv, rf"LATCH_MAGIC = 16'h{spec['latch_magic']:04X}"),
    (sv, rf"STATUS_MAGIC = 16'h{spec['status_magic']:04X}"),
    (sv, rf"CAPS_MAGIC = 16'h{spec['caps_magic']:04X}"),
]
for text, pattern in checks:
    if re.search(pattern, text) is None:
        raise SystemExit(f"latch protocol generated contract mismatch: {pattern}")
if len(spec["status_words"]) != 11 or "STATUS_WORD_COUNT: usize = 11" not in rust:
    raise SystemExit("latch status word schema mismatch")
if len(spec["capability_words"]) != 5 or "CAPS_WORD_COUNT: usize = 5" not in rust:
    raise SystemExit("latch capability word schema mismatch")
print("latch protocol contract: ok")
