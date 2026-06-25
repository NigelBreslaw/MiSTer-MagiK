#!/usr/bin/env python3
"""Compare two frame-profile TSVs phase by phase.

Usage:
  scripts/frame-profile-compare.py before.tsv after.tsv
"""

from __future__ import annotations

import argparse
import csv
from pathlib import Path


PHASES = [
    "wall_us",
    "prepare_us",
    "anim_us",
    "slint_render_us",
    "custom_draw_us",
    "vsync_us",
    "fb_present_us",
    "cached_present_us",
    "arcade_list_present_us",
    "present_pixels",
    "present_bytes",
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


def percentile(values: list[int], pct: float) -> int:
    if not values:
        return 0
    sorted_values = sorted(values)
    idx = round((len(sorted_values) - 1) * pct / 100.0)
    return sorted_values[min(len(sorted_values) - 1, idx)]


def stats(rows: list[dict[str, str]], key: str) -> dict[str, int]:
    values = [int_field(row, key) for row in rows]
    if not values:
        return {"avg": 0, "p50": 0, "p95": 0, "max": 0}
    return {
        "avg": sum(values) // len(values),
        "p50": percentile(values, 50),
        "p95": percentile(values, 95),
        "max": max(values),
    }


def fmt_delta(delta: int) -> str:
    if delta > 0:
        return f"+{delta}"
    return str(delta)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("before", type=Path)
    parser.add_argument("after", type=Path)
    args = parser.parse_args()

    before = read_rows(args.before)
    after = read_rows(args.after)
    print(f"before={args.before} frames={len(before)}")
    print(f"after ={args.after} frames={len(after)}")
    print("metric\tbefore_avg\tafter_avg\tdelta_avg\tbefore_p50\tafter_p50\tdelta_p50\tbefore_p95\tafter_p95\tdelta_p95")
    for phase in PHASES:
        b = stats(before, phase)
        a = stats(after, phase)
        print(
            f"{phase}\t{b['avg']}\t{a['avg']}\t{fmt_delta(a['avg'] - b['avg'])}"
            f"\t{b['p50']}\t{a['p50']}\t{fmt_delta(a['p50'] - b['p50'])}"
            f"\t{b['p95']}\t{a['p95']}\t{fmt_delta(a['p95'] - b['p95'])}"
        )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
