#!/usr/bin/env python3
"""Compare repeated launcher work-p99 traces for scanout qualification."""

from __future__ import annotations

import argparse
import csv
import math
import statistics
import sys
from pathlib import Path


def integer(row: dict[str, str], key: str) -> int:
    try:
        return int(float(row.get(key, "") or 0))
    except ValueError:
        return 0


def trace_metrics(path: Path) -> tuple[int, int, int]:
    with path.open(newline="") as source:
        rows = [row for row in csv.DictReader(source, delimiter="\t") if integer(row, "frame") > 30]
    if not rows:
        raise ValueError(f"{path}: no measured frames")
    work = sorted(
        integer(row, "prepare_us")
        + integer(row, "slint_render_us")
        + integer(row, "custom_draw_us")
        + integer(row, "fb_present_us")
        for row in rows
    )
    index = max(0, min(len(work) - 1, math.ceil(len(work) * 0.99) - 1))
    violations = sum(
        1
        for row in rows
        if row.get("vsync_source") != "vsync"
        or integer(row, "vsync_miss_streak") != 0
        or row.get("main_present_status") not in ("", "ok")
    )
    return len(rows), work[index], violations


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--scene", required=True)
    parser.add_argument("--before", action="append", type=Path, required=True)
    parser.add_argument("--after", action="append", type=Path, default=[])
    args = parser.parse_args()

    before = [trace_metrics(path) for path in args.before]
    after = [trace_metrics(path) for path in args.after]
    before_median = int(statistics.median(metric[1] for metric in before))
    valid = all(metric[2] == 0 for metric in before + after)
    if after:
        valid = valid and all(metric[1] < before_median for metric in after)
    print(
        "scanout_comparison_tsv"
        f"\tscene={args.scene}"
        f"\tvalid={int(valid)}"
        f"\tbefore_p99={','.join(str(metric[1]) for metric in before)}"
        f"\tbefore_median={before_median}"
        f"\tafter_p99={','.join(str(metric[1]) for metric in after) or 'pending'}"
        f"\tintegrity_violations={sum(metric[2] for metric in before + after)}"
    )
    return 0 if valid else 1


if __name__ == "__main__":
    sys.exit(main())
