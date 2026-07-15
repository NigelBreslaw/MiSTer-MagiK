#!/usr/bin/env python3
# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

"""Bounded host model for the pinned scanout platform admission policy."""

from pathlib import Path
import re
from typing import List, Optional, Tuple

ROOT = Path(__file__).resolve().parents[1]
HEADER = ROOT / "kernel/scanout-slots/mister_magik_scanout_platform.h"


def number(name: str) -> int:
    match = re.search(rf"^#define {name} (0x[0-9a-fA-F]+|[0-9]+)UL$", HEADER.read_text(), re.M)
    assert match, name
    return int(match.group(1), 0)


VISIBLE_BASE = number("MISTER_MAGIK_PLATFORM_FB_VISIBLE_BASE")
SLOTS = (number("MISTER_MAGIK_PLATFORM_SLOT0_PHYS"), number("MISTER_MAGIK_PLATFORM_SLOT1_PHYS"))
MAP_BYTES = number("MISTER_MAGIK_PLATFORM_MAP_BYTES")
MAX_PHYS = (1 << 32) - 1


def text(name: str) -> str:
    match = re.search(rf'^#define {name} "([^"]+)"$', HEADER.read_text(), re.M)
    assert match, name
    return match.group(1)


KERNEL_RELEASE = text("MISTER_MAGIK_PLATFORM_KERNEL_RELEASE")
MACHINE = text("MISTER_MAGIK_PLATFORM_MACHINE")
FB_ID = text("MISTER_MAGIK_PLATFORM_FB_ID")


def overlaps(start: int, size: int, other_start: int, other_size: int) -> bool:
    end = start + size - 1
    other_end = other_start + other_size - 1
    return start <= other_end and other_start <= end


def accepts(*, kernel_release=KERNEL_RELEASE, machine=MACHINE, fb_id=FB_ID,
            fb_start=VISIBLE_BASE, fb_len=MAP_BYTES, slots=SLOTS, ram=(), occupied=()) -> bool:
    if (kernel_release != KERNEL_RELEASE or machine != MACHINE or fb_id != FB_ID
            or fb_start != VISIBLE_BASE or fb_len <= 0):
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


def reserve_with_rollback(fail_index: Optional[int] = None) -> Tuple[bool, Tuple[int, ...]]:
    reserved: List[int] = []
    for index, slot in enumerate(SLOTS):
        if index == fail_index:
            reserved.clear()
            return False, tuple(reserved)
        reserved.append(slot)
    return True, tuple(reserved)


def main() -> None:
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
