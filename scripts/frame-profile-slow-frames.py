#!/usr/bin/env python3
"""Print the slowest frames from a frame-profile TSV.

Usage:
  scripts/frame-profile-slow-frames.py /tmp/frames.tsv --limit 12
"""

from __future__ import annotations

import argparse
import csv
from pathlib import Path


PHASES = [
    "prepare_us",
    "anim_us",
    "slint_render_us",
    "custom_draw_us",
    "vsync_us",
    "cached_present_us",
    "arcade_list_present_us",
]


def int_field(row: dict[str, str], key: str) -> int:
    if key == "arcade_list_present_us" and key not in row:
        key = "overlay_present_us"
    try:
        return int(float(row.get(key, "") or 0))
    except ValueError:
        return 0


def read_rows(path: Path) -> list[dict[str, str]]:
    with path.open(newline="") as f:
        return list(csv.DictReader(f, delimiter="\t"))


def rect_label(row: dict[str, str]) -> str:
    x0 = int_field(row, "present_x0")
    y0 = int_field(row, "present_y0")
    x1 = int_field(row, "present_x1")
    y1 = int_field(row, "present_y1")
    if x1 <= x0 or y1 <= y0:
        return "none"
    return f"{x0},{y0}..{x1},{y1}"


def dominant_phase(row: dict[str, str]) -> str:
    label = row.get("dominant", "")
    if label:
        return label
    return max(PHASES, key=lambda phase: int_field(row, phase)).replace("_us", "")


def phase_summary(row: dict[str, str]) -> str:
    parts = []
    for phase in PHASES:
        value = int_field(row, phase)
        if value:
            parts.append(f"{phase.replace('_us', '')}={value}us")
    return " ".join(parts) or "no phase time"


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("input", type=Path, help="frame profile TSV")
    parser.add_argument("--limit", type=int, default=10)
    parser.add_argument("--threshold-us", type=int, default=16_667)
    args = parser.parse_args()

    rows = read_rows(args.input)
    indexed = list(enumerate(rows))
    indexed.sort(key=lambda item: int_field(item[1], "wall_us"), reverse=True)
    slow_count = sum(1 for row in rows if int_field(row, "wall_us") >= args.threshold_us)
    print(
        f"{args.input}: frames={len(rows)} slow>={args.threshold_us}us={slow_count} "
        f"showing={min(args.limit, len(rows))}"
    )
    for idx, row in indexed[: args.limit]:
        wall = int_field(row, "wall_us")
        pixels = int_field(row, "present_pixels")
        bytes_ = int_field(row, "present_bytes")
        print(
            f"#{idx} wall={wall}us dominant={dominant_phase(row)} "
            f"rect={rect_label(row)} pixels={pixels} bytes={bytes_} rows={int_field(row, 'rows')}"
        )
        print(f"  {phase_summary(row)}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
