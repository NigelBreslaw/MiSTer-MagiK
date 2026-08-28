#!/usr/bin/env python3
# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

"""Validate launcher frame pacing traces and emit frame_pacing_gate_tsv."""

from __future__ import annotations

import csv
import math
import sys

REQUIRED_COLUMNS = {
    "frame",
    "wall_us",
    "prepare_us",
    "slint_render_us",
    "custom_draw_us",
    "fb_present_us",
    "vsync_source",
    "vsync_miss_streak",
}


def int_field(row: dict[str, str], key: str) -> int:
    try:
        return int(float(row.get(key, "") or 0))
    except ValueError:
        return 0


def fail(label: str, reason: str, detail: str) -> int:
    print(
        f"frame_pacing_gate_tsv\tlabel={label}\tvalid=0"
        f"\tinvalid_reason={reason}\tdetail={detail}"
    )
    return 9


def check_frame_pacing(
    label: str,
    trace_path: str,
    p99_work_us: int,
    p99_wall_us: int,
    max_wall_us: int,
    scenario: str = "",
    policy: str = "auto",
) -> int:
    try:
        with open(trace_path, newline="") as f:
            rows = list(csv.DictReader(f, delimiter="\t"))
    except FileNotFoundError:
        return fail(label, "missing_trace", trace_path)

    if not rows:
        return fail(label, "no_frames", trace_path)

    missing = sorted(REQUIRED_COLUMNS - set(rows[0]))
    if missing:
        return fail(label, "missing_column", ",".join(missing))

    measured: list[dict[str, str]] = []
    for row in rows:
        source = row.get("vsync_source", "")
        miss = int_field(row, "vsync_miss_streak")
        if not measured and source in ("", "none") and miss == 0:
            continue
        if int_field(row, "frame") <= 30:
            continue
        measured.append(row)

    if not measured:
        return fail(label, "no_measured_frames", trace_path)

    works = sorted(
        int_field(row, "prepare_us")
        + int_field(row, "slint_render_us")
        + int_field(row, "custom_draw_us")
        + int_field(row, "hidden_compose_us")
        + int_field(row, "fb_present_us")
        for row in measured
    )
    p99_index = max(0, min(len(works) - 1, math.ceil(len(works) * 0.99) - 1))
    p99_work = works[p99_index]
    walls = sorted(int_field(row, "wall_us") for row in measured)
    p99_wall = walls[p99_index]
    max_wall = walls[-1]
    work_over = sum(1 for value in works if value > 16667)
    wall_over = sum(1 for value in walls if value > 16667)
    wall_over_18ms = sum(1 for value in walls if value > 18000)
    wall_over_20ms = sum(1 for value in walls if value > 20000)
    wall_over_33ms = sum(1 for value in walls if value > 33334)
    low_work_high_wall = 0
    sources = {"vsync": 0, "fallback": 0, "timeout": 0, "error": 0, "other_source": 0}
    dropped_rows = 0
    max_miss = 0
    examples: list[str] = []

    for row in measured:
        work = (
            int_field(row, "prepare_us")
            + int_field(row, "slint_render_us")
            + int_field(row, "custom_draw_us")
            + int_field(row, "hidden_compose_us")
            + int_field(row, "fb_present_us")
        )
        if int_field(row, "wall_us") > 16667 and work <= 16667:
            low_work_high_wall += 1

        source = row.get("vsync_source", "")
        if source in ("vsync", "fallback", "timeout", "error"):
            sources[source] += 1
        else:
            sources["other_source"] += 1

        miss = int_field(row, "vsync_miss_streak")
        max_miss = max(max_miss, miss)
        if source != "vsync" or miss > 0:
            dropped_rows += 1
            if len(examples) < 3:
                examples.append(
                    f"frame={row.get('frame', '?')}:source={source or 'blank'}:miss={miss}"
                )

    if policy not in ("auto", "strict", "vsync-integrity"):
        return fail(label, "invalid_policy", policy)
    wall_policy = policy
    if wall_policy == "auto":
        wall_policy = "vsync-integrity" if scenario == "human-turbo-hold" else "strict"
    common_valid = (
        p99_work <= p99_work_us
        and work_over == 0
        and sources["fallback"] == 0
        and sources["timeout"] == 0
        and sources["error"] == 0
        and sources["other_source"] == 0
        and max_miss == 0
    )
    if wall_policy == "vsync-integrity":
        wall_valid = wall_over_33ms == 0
    else:
        wall_valid = (
            p99_wall <= p99_wall_us and max_wall <= max_wall_us and wall_over == 0
        )
    valid = common_valid and wall_valid
    detail = (
        f"frames_after_30={len(measured)} scenario={scenario or 'default'} "
        f"wall_policy={wall_policy} "
        f"p99_work_us={p99_work} work_threshold={p99_work_us} "
        f"p99_wall_us={p99_wall} wall_p99_threshold={p99_wall_us} max_wall_us={max_wall} "
        f"wall_max_threshold={max_wall_us} work_gt_16667={work_over} wall_gt_16667={wall_over} "
        f"wall_gt_18000={wall_over_18ms} wall_gt_20000={wall_over_20ms} "
        f"wall_gt_33334={wall_over_33ms} low_work_high_wall={low_work_high_wall} "
        f"dropped_rows={dropped_rows} vsync={sources['vsync']} "
        f"fallback={sources['fallback']} timeout={sources['timeout']} error={sources['error']} "
        f"other_source={sources['other_source']} max_miss_streak={max_miss}"
    )
    if examples:
        detail += " " + " ".join(examples)
    print(
        f"frame_pacing_gate_tsv\tlabel={label}\tvalid={1 if valid else 0}"
        f"\tinvalid_reason={'ok' if valid else 'gate_failed'}\tdetail={detail}"
    )
    return 0 if valid else 9


def main(argv: list[str]) -> int:
    if len(argv) not in (6, 7, 8):
        print(
            "usage: check-frame-pacing-trace.py LABEL TRACE P99_WORK_US P99_WALL_US MAX_WALL_US [SCENARIO] [POLICY]",
            file=sys.stderr,
        )
        return 2
    label, trace_path, p99_work_us_text, p99_wall_us_text, max_wall_us_text = argv[1:6]
    scenario = argv[6] if len(argv) == 7 else ""
    if len(argv) == 8:
        scenario = argv[6]
    policy = argv[7] if len(argv) == 8 else "auto"
    return check_frame_pacing(
        label,
        trace_path,
        int(p99_work_us_text),
        int(p99_wall_us_text),
        int(max_wall_us_text),
        scenario,
        policy,
    )


if __name__ == "__main__":
    sys.exit(main(sys.argv))
