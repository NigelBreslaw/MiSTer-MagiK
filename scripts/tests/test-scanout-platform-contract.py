#!/usr/bin/env python3
# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

"""Bounded host model for the pinned scanout platform admission policy."""

from __future__ import annotations

import re
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
HEADER = ROOT / "mister/platform/kernel/scanout-slots/mister_magik_scanout_platform.h"
RUST_CONTRACT = ROOT / "mister/platform/contracts/scanout/src/lib.rs"
LATCH_PROTOCOL = (
    ROOT / "mister/platform/fpga/menu-vblank-latch/mister_magik_latch_protocol.svh"
)


def number(name: str) -> int:
    match = re.search(
        rf"^#define {name} (0x[0-9a-fA-F]+|[0-9]+)UL$", HEADER.read_text(), re.MULTILINE
    )
    assert match, name
    return int(match.group(1), 0)


def rust_number(name: str) -> int:
    match = re.search(
        rf"^pub const {name}: (?:usize|u32) = ([0-9_]+);$",
        RUST_CONTRACT.read_text(),
        re.MULTILINE,
    )
    assert match, name
    return int(match.group(1).replace("_", ""))


def sv_number(name: str) -> int:
    match = re.search(
        rf"^localparam \[15:0\] {name} = 16'd([0-9]+);$",
        LATCH_PROTOCOL.read_text(),
        re.MULTILINE,
    )
    assert match, name
    return int(match.group(1))


VISIBLE_BASE = number("MISTER_MAGIK_PLATFORM_FB_VISIBLE_BASE")
SLOTS = (
    number("MISTER_MAGIK_PLATFORM_SLOT0_PHYS"),
    number("MISTER_MAGIK_PLATFORM_SLOT1_PHYS"),
)
MAP_BYTES = number("MISTER_MAGIK_PLATFORM_MAP_BYTES")
MAX_WIDTH = number("MISTER_MAGIK_PLATFORM_MAX_WIDTH")
MAX_HEIGHT = number("MISTER_MAGIK_PLATFORM_MAX_HEIGHT")
MAX_STRIDE = number("MISTER_MAGIK_PLATFORM_MAX_STRIDE_BYTES")
CAPACITY = number("MISTER_MAGIK_PLATFORM_SLOT_CAPACITY_BYTES")
ABI_VERSION = number("MISTER_MAGIK_PLATFORM_ABI_VERSION")
MAX_PHYS = (1 << 32) - 1


def text(name: str) -> str:
    match = re.search(rf'^#define {name} "([^"]+)"$', HEADER.read_text(), re.MULTILINE)
    assert match, name
    return match.group(1)


KERNEL_RELEASE = text("MISTER_MAGIK_PLATFORM_KERNEL_RELEASE")
MACHINE = text("MISTER_MAGIK_PLATFORM_MACHINE")
FB_ID = text("MISTER_MAGIK_PLATFORM_FB_ID")


def overlaps(start: int, size: int, other_start: int, other_size: int) -> bool:
    end = start + size - 1
    other_end = other_start + other_size - 1
    return start <= other_end and other_start <= end


def accepts(
    *,
    kernel_release=KERNEL_RELEASE,
    machine=MACHINE,
    fb_id=FB_ID,
    fb_start=VISIBLE_BASE,
    fb_len=MAP_BYTES,
    slots=SLOTS,
    ram=(),
    occupied=(),
) -> bool:
    if (
        kernel_release != KERNEL_RELEASE
        or machine != MACHINE
        or fb_id != FB_ID
        or fb_start != VISIBLE_BASE
        or fb_len <= 0
    ):
        return False
    if fb_start + fb_len - 1 > slots[0] - 1:
        return False
    ends = []
    for slot in slots:
        end = slot + MAP_BYTES - 1
        if slot < 0 or end > MAX_PHYS or end < slot:
            return False
        if any(overlaps(slot, MAP_BYTES, start, size) for start, size in ram):
            return False
        if any(overlaps(slot, MAP_BYTES, start, size) for start, size in occupied):
            return False
        ends.append(end)
    return ends[0] < slots[1]


def reserve_with_rollback(
    fail_index: int | None = None,
) -> tuple[bool, tuple[int, ...]]:
    reserved: list[int] = []
    for index, slot in enumerate(SLOTS):
        if index == fail_index:
            reserved.clear()
            return False, tuple(reserved)
        reserved.append(slot)
    return True, tuple(reserved)


def main() -> None:
    assert text("MISTER_MAGIK_PLATFORM_CONTRACT_ID") == "mister-5.15.1-scanout-v3"
    assert (MAX_WIDTH, MAX_HEIGHT, MAX_STRIDE, CAPACITY, MAP_BYTES, ABI_VERSION) == (
        1366,
        768,
        2736,
        2_101_248,
        2_101_248,
        3,
    )
    assert MAX_STRIDE == ((MAX_WIDTH * 2 + 15) & ~15)
    assert CAPACITY == MAX_STRIDE * MAX_HEIGHT
    assert MAP_BYTES % 4096 == 0
    assert SLOTS[0] + MAP_BYTES <= SLOTS[1]
    assert (
        rust_number("MAX_WIDTH"),
        rust_number("MAX_HEIGHT"),
        rust_number("MAX_STRIDE_BYTES"),
        rust_number("ABI_VERSION"),
    ) == (MAX_WIDTH, MAX_HEIGHT, MAX_STRIDE, ABI_VERSION)
    assert (
        sv_number("MAGIK_FBUF_MAX_WIDTH"),
        sv_number("MAGIK_FBUF_MAX_HEIGHT"),
        sv_number("MAGIK_FBUF_MAX_STRIDE"),
    ) == (MAX_WIDTH, MAX_HEIGHT, MAX_STRIDE)
    assert accepts()
    assert not accepts(kernel_release="5.15.1-other")
    assert not accepts(machine="other-machine")
    assert not accepts(fb_id="other-fb")
    assert not accepts(fb_start=VISIBLE_BASE - 0x1000)
    assert not accepts(fb_len=SLOTS[0] - VISIBLE_BASE + 1)
    assert not accepts(slots=(SLOTS[0], SLOTS[0] + MAP_BYTES - 1))
    assert not accepts(slots=(SLOTS[0], MAX_PHYS - MAP_BYTES + 2))
    assert not accepts(ram=((SLOTS[0], 0x1000),))
    assert not accepts(ram=((SLOTS[1] + MAP_BYTES - 1, 0x1000),))
    assert not accepts(occupied=((SLOTS[0] + 0x1000, 0x1000),))
    assert not accepts(occupied=((SLOTS[1], MAP_BYTES),))
    assert reserve_with_rollback() == (True, SLOTS)
    assert reserve_with_rollback(0) == (False, ())
    assert reserve_with_rollback(1) == (False, ())
    print("scanout platform contract model: ok cases=15")


if __name__ == "__main__":
    main()
