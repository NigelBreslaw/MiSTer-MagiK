#!/usr/bin/env python3
"""Print phase histograms from a frame-profile TSV.

Usage:
  scripts/frame-profile-histogram.py /tmp/frames.tsv
"""

from __future__ import annotations

import argparse
import csv
from pathlib import Path


DEFAULT_PHASES = [
    "wall_us",
    "slint_render_us",
    "vsync_us",
    "fb_present_us",
    "cached_present_us",
    "direct_preview_present_us",
    "arcade_list_present_us",
    "video_decode_us",
    "video_scale_us",
    "video_image_us",
    "video_blit_us",
    "audio_decode_us",
    "audio_resample_us",
    "audio_write_us",
]

BUCKETS_US = [
    (0, 100, "[0,100us)"),
    (100, 500, "[100,500us)"),
    (500, 1_000, "[0.5,1ms)"),
    (1_000, 2_000, "[1,2ms)"),
    (2_000, 5_000, "[2,5ms)"),
    (5_000, 10_000, "[5,10ms)"),
    (10_000, 15_000, "[10,15ms)"),
    (15_000, 17_000, "[15,17ms)"),
    (17_000, 30_000, "[17,30ms)"),
    (30_000, 1_000_000_000, "[30ms,+)"),
]


def int_field(row: dict[str, str], key: str) -> int:
    if key == "arcade_list_present_us" and key not in row:
        key = "overlay_present_us"
    try:
        return int(float(row.get(key, "") or 0))
    except ValueError:
        return 0


def percentile(sorted_values: list[int], pct: float) -> int:
    if not sorted_values:
        return 0
    idx = round((len(sorted_values) - 1) * pct / 100.0)
    return sorted_values[min(len(sorted_values) - 1, idx)]


def read_rows(path: Path) -> list[dict[str, str]]:
    with path.open(newline="") as f:
        return list(csv.DictReader(f, delimiter="\t"))


def histogram(values: list[int]) -> list[tuple[str, int]]:
    out: list[tuple[str, int]] = []
    for low, high, label in BUCKETS_US:
        out.append((label, sum(1 for value in values if low <= value < high)))
    return out


def print_phase(rows: list[dict[str, str]], phase: str, width: int) -> None:
    values = sorted(int_field(row, phase) for row in rows)
    if not values:
        return
    total = len(values)
    avg = sum(values) // total
    print(
        f"{phase}: min={values[0]} p50={percentile(values, 50)} "
        f"p95={percentile(values, 95)} p99={percentile(values, 99)} "
        f"max={values[-1]} avg={avg}"
    )
    max_count = max(1, max(count for _, count in histogram(values)))
    for label, count in histogram(values):
        if count == 0:
            continue
        bar = "#" * max(1, round(count * width / max_count))
        print(f"  {label:10} {count:5d} {bar}")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("input", type=Path, help="frame profile TSV")
    parser.add_argument("--phase", action="append", dest="phases", help="phase column to print")
    parser.add_argument("--width", type=int, default=48, help="maximum bar width")
    args = parser.parse_args()

    rows = read_rows(args.input)
    phases = args.phases or DEFAULT_PHASES
    print(f"{args.input}: {len(rows)} frames")
    for idx, phase in enumerate(phases):
        if idx:
            print()
        print_phase(rows, phase, args.width)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
