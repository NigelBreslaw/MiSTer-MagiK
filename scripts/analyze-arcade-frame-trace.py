#!/usr/bin/env python3
"""Summarize real arcade-screen frame traces.

The current trace is produced by `MISTER_PREVIEW_SCROLL_TRACE` from the
Main-supervised launcher Arcade screen and has one row per frame. Older
`MISTER_ARCADE_FRAME_TRACE` files are still accepted so historical captures
remain readable. By default the first frame is ignored because it includes
startup rendering.
"""

from __future__ import annotations

import argparse
import csv
from pathlib import Path


FLOAT_COLUMNS = {"visual_index", "transition_progress"}
STRING_COLUMNS = {
    "update",
    "arcade_update",
    "cache_state",
    "transition_effect",
    "vsync_source",
}
PHASES = [
    "prepare_us",
    "slint_render_us",
    "custom_draw_us",
    "vsync_us",
    "fb_present_us",
    "cached_present_us",
    "arcade_list_present_us",
    "wall_us",
    "rows",
]


def parse_value(key: str, value: str) -> int | float | str:
    if key in STRING_COLUMNS:
        return value
    if key in FLOAT_COLUMNS:
        return float(value)
    return int(value)


def percentile(values: list[int], pct: int) -> int:
    if not values:
        return 0
    values = sorted(values)
    idx = round((len(values) - 1) * pct / 100)
    return values[idx]


def print_stats(label: str, rows: list[dict[str, int | float | str]]) -> None:
    print(f"{label}: frames={len(rows)}")
    if not rows:
        return
    for phase in PHASES:
        if phase not in rows[0]:
            continue
        values = [int(row[phase]) for row in rows]
        avg = sum(values) / len(values)
        print(
            f"  {phase:18s}"
            f" p50={percentile(values, 50):5d}"
            f" p95={percentile(values, 95):5d}"
            f" p99={percentile(values, 99):5d}"
            f" max={max(values):5d}"
            f" avg={avg:8.1f}"
        )


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("trace", type=Path)
    parser.add_argument(
        "--include-first",
        action="store_true",
        help="include frame 0 instead of treating it as startup warmup",
    )
    parser.add_argument("--worst", type=int, default=8)
    args = parser.parse_args()

    with args.trace.open(newline="") as f:
        rows = [
            {key: parse_value(key, value) for key, value in row.items()}
            for row in csv.DictReader(f, delimiter="\t")
        ]
    for row in rows:
        if "arcade_update" in row and "update" not in row:
            row["update"] = row["arcade_update"]
        if "arcade_draw_us" in row and "custom_draw_us" not in row:
            row["custom_draw_us"] = row["arcade_draw_us"]
        if "overlay_present_us" in row and "arcade_list_present_us" not in row:
            row["arcade_list_present_us"] = row["overlay_present_us"]
    if not args.include_first:
        rows = [row for row in rows if int(row["frame"]) != 0]

    print(f"{args.trace}: frames={len(rows)} include_first={args.include_first}")
    print_stats("all", rows)
    print()
    for update in sorted({str(row["update"]) for row in rows}):
        group = [row for row in rows if row["update"] == update]
        print_stats(f"update={update}", group)
        print()

    if rows and "transition_effect" in rows[0]:
        for effect in sorted({str(row["transition_effect"]) for row in rows}):
            group = [row for row in rows if row["transition_effect"] == effect]
            print_stats(f"transition_effect={effect}", group)
            print()

    print(f"worst wall_us ({args.worst})")
    for row in sorted(rows, key=lambda item: int(item["wall_us"]), reverse=True)[: args.worst]:
        fields = [
            "frame",
            "selected",
            "visual_index",
            "transition_effect",
            "transition_progress",
            "update",
            "rows",
            "prepare_us",
            "slint_render_us",
            "custom_draw_us",
            "fb_present_us",
            "cached_present_us",
            "arcade_list_present_us",
            "wall_us",
        ]
        print("\t".join(f"{field}={row[field]}" for field in fields if field in row))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
