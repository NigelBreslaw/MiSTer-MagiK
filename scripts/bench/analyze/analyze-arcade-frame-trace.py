#!/usr/bin/env python3
# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

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
import json
import tempfile
from pathlib import Path

FLOAT_COLUMNS = {"visual_index", "transition_progress"}
STRING_COLUMNS = {
    "update",
    "arcade_update",
    "home_screen",
    "cache_state",
    "transition_effect",
    "preview_fade_path",
    "main_present_backend",
    "main_present_status",
    "vsync_source",
    "search_index_state",
}
PHASES = [
    "prepare_us",
    "slint_render_us",
    "custom_draw_us",
    "vsync_us",
    "fb_present_us",
    "cached_present_us",
    "hidden_compose_us",
    "hidden_preview_compose_us",
    "hidden_arcade_compose_us",
    "direct_preview_present_us",
    "arcade_list_present_us",
    "main_present_hidden_invalid_bytes",
    "main_present_hidden_rect_count",
    "main_present_hidden_catchup_bytes",
    "main_present_hidden_full_copy",
    "main_present_set_vga_fb_us",
    "present_bytes",
    "wasted_present_bytes",
    "wall_us",
    "rows",
]
WINDOWS_US = [
    ("0-3s", 0, 3_000_000),
    ("3-10s", 3_000_000, 10_000_000),
    ("10-30s", 10_000_000, 30_000_000),
]
INTERRUPTION_COLUMNS = [
    "catalog_worker_us",
    "catalog_message_count",
    "catalog_backlog",
    "catalog_ready_deferred",
    "media_worker_us",
    "media_gate_us",
    "preview_schedule_us",
    "preview_apply_us",
    "status_write_due",
    "runtime_status_write_deferred",
    "frame_tail_slack_us",
    "status_string_copy_us",
    "runtime_status_write_us",
    "status_write_duration_us",
    "dirty_y0",
    "dirty_y1",
    "vsync_stale_hits",
    "vsync_wait_start_age_us",
    "vsync_accepted_hit_age_us",
    "frame_start_phase_us",
    "present_phase_us",
]
DOMINANT_DELTA_COLUMNS = [
    "vsync_us",
    "fb_present_us",
    "hidden_compose_us",
    "hidden_preview_compose_us",
    "hidden_arcade_compose_us",
    "direct_preview_present_us",
    "arcade_list_present_us",
    "runtime_status_write_us",
    "status_write_duration_us",
    "preview_apply_us",
    "preview_cache_inserts",
    "preview_cache_evictions",
    "vsync_wait_start_age_us",
    "frame_start_phase_us",
    "present_phase_us",
]


def parse_value(key: str, value: str) -> int | float | str:
    if key in STRING_COLUMNS:
        return value
    if key in FLOAT_COLUMNS:
        return float(value)
    return int(value)


def normalize_row(row: dict[str, int | float | str]) -> None:
    if "arcade_update" in row and "update" not in row:
        row["update"] = row["arcade_update"]
    if "arcade_draw_us" in row and "custom_draw_us" not in row:
        row["custom_draw_us"] = row["arcade_draw_us"]
    if "overlay_present_us" in row and "arcade_list_present_us" not in row:
        row["arcade_list_present_us"] = row["overlay_present_us"]
    for key in (
        "hidden_compose_us",
        "hidden_preview_compose_us",
        "hidden_arcade_compose_us",
        "direct_preview_present_us",
        "arcade_list_present_us",
        "main_present_hidden_invalid_bytes",
        "main_present_hidden_rect_count",
        "main_present_hidden_catchup_bytes",
        "main_present_hidden_full_copy",
        "main_present_set_vga_fb_us",
    ):
        row.setdefault(key, 0)


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


def row_time_us(row: dict[str, int | float | str]) -> int:
    if "elapsed_us" in row:
        return int(row["elapsed_us"])
    return int(row["frame"]) * 16_667


def work_us(row: dict[str, int | float | str]) -> int:
    return (
        int(row.get("prepare_us", 0))
        + int(row.get("slint_render_us", 0))
        + int(row.get("custom_draw_us", 0))
        + int(row.get("hidden_compose_us", 0))
        + int(row.get("fb_present_us", 0))
    )


def print_smaller_stutter_report(rows: list[dict[str, int | float | str]]) -> None:
    print("smaller stutter buckets")
    if not rows:
        print("  unavailable")
        return
    counts = [
        ("wall_gt_16_7ms", sum(1 for row in rows if int(row["wall_us"]) > 16_667)),
        ("wall_gt_18ms", sum(1 for row in rows if int(row["wall_us"]) > 18_000)),
        ("wall_gt_20ms", sum(1 for row in rows if int(row["wall_us"]) > 20_000)),
        ("wall_gt_33ms", sum(1 for row in rows if int(row["wall_us"]) > 33_334)),
        (
            "low_work_high_wall",
            sum(
                1
                for row in rows
                if int(row["wall_us"]) > 16_667 and work_us(row) <= 16_667
            ),
        ),
        ("work_gt_16_7ms", sum(1 for row in rows if work_us(row) > 16_667)),
    ]
    print("  " + " ".join(f"{name}={value}" for name, value in counts))
    slow_rows = [row for row in rows if int(row["wall_us"]) > 16_667]
    if not slow_rows:
        print("  dominant_deltas: none")
        return
    medians = {
        column: percentile([int(row.get(column, 0)) for row in rows], 50)
        for column in DOMINANT_DELTA_COLUMNS
        if column in rows[0]
    }
    dominant: dict[str, int] = {}
    for row in slow_rows:
        best_column = "none"
        best_delta = 0
        for column, median in medians.items():
            delta = int(row.get(column, 0)) - median
            if delta > best_delta:
                best_column = column
                best_delta = delta
        dominant[best_column] = dominant.get(best_column, 0) + 1
    ranked = sorted(dominant.items(), key=lambda item: (-item[1], item[0]))
    print("  dominant_deltas: " + " ".join(f"{name}={count}" for name, count in ranked))
    severe_rows = [row for row in rows if int(row["wall_us"]) > 20_000]
    if severe_rows:
        print_interruption_summary("  wall_gt_20ms correlation", severe_rows)


def print_window_report(rows: list[dict[str, int | float | str]]) -> None:
    print("first 30s windows")
    for label, start_us, end_us in WINDOWS_US:
        group = [row for row in rows if start_us <= row_time_us(row) < end_us]
        if not group:
            print(f"  {label}: frames=0")
            continue
        walls = [int(row["wall_us"]) for row in group]
        works = [work_us(row) for row in group]
        warnings = [row for row in group if int(row["wall_us"]) >= 16_000]
        overruns = [row for row in group if int(row["wall_us"]) > 16_667]
        print(
            f"  {label}: frames={len(group)} "
            f"wall_p99={percentile(walls, 99)} wall_max={max(walls)} "
            f"work_p99={percentile(works, 99)} work_max={max(works)} "
            f"cadence_warnings_ge_16ms={len(warnings)} "
            f"wall_overruns_gt_16_7ms={len(overruns)}"
        )
        if warnings:
            print_interruption_summary(
                f"    cadence-warning correlation {label}", warnings
            )


def print_interruption_summary(
    label: str, rows: list[dict[str, int | float | str]], *, top: int = 4
) -> None:
    print(label)
    if not rows:
        print("      none")
        return
    for column in INTERRUPTION_COLUMNS:
        if column not in rows[0]:
            continue
        values = [int(row[column]) for row in rows]
        total = sum(values)
        maximum = max(values)
        active = sum(1 for value in values if value > 0)
        if total == 0 and maximum == 0:
            continue
        print(
            f"      {column}: active_frames={active} "
            f"sum={total} p95={percentile(values, 95)} max={maximum}"
        )
    print("      worst_frames:")
    for row in sorted(rows, key=lambda item: int(item["wall_us"]), reverse=True)[:top]:
        fields = [
            f"frame={row.get('frame', '-')}",
            f"elapsed_us={row_time_us(row)}",
            f"wall_us={row.get('wall_us', 0)}",
            f"work_us={work_us(row)}",
            f"catalog_messages={row.get('catalog_message_count', 0)}",
            f"catalog_backlog={row.get('catalog_backlog', 0)}",
            f"media_worker_us={row.get('media_worker_us', 0)}",
            f"preview_apply_us={row.get('preview_apply_us', 0)}",
            f"status_due={row.get('status_write_due', 0)}",
            f"status_deferred={row.get('runtime_status_write_deferred', 0)}",
            f"frame_tail_slack_us={row.get('frame_tail_slack_us', 0)}",
            f"runtime_status_write_us={row.get('runtime_status_write_us', 0)}",
            f"dirty_y={row.get('dirty_y0', 0)}-{row.get('dirty_y1', 0)}",
        ]
        print("        " + " ".join(fields))


def status_slow_frames(status_path: Path | None) -> list[dict[str, object]]:
    if (
        status_path is None
        or not status_path.exists()
        or status_path.stat().st_size == 0
    ):
        return []
    with status_path.open() as f:
        status = json.load(f)
    launcher = status.get("launcher", {}) if isinstance(status, dict) else {}
    frame_budget = (
        launcher.get("frame_budget", {}) if isinstance(launcher, dict) else {}
    )
    slow = frame_budget.get("slow_frames", []) if isinstance(frame_budget, dict) else []
    return [row for row in slow if isinstance(row, dict)]


def print_status_tombstones(rows: list[dict[str, object]]) -> None:
    print("runtime tombstones")
    if not rows:
        print("  unavailable")
        return
    for row in sorted(rows, key=lambda item: int(item.get("wall_us", 0)), reverse=True)[
        :8
    ]:
        fields = [
            f"frame={row.get('frame', '-')}",
            f"severity={row.get('severity', '-')}",
            f"wall_us={row.get('wall_us', 0)}",
            f"dominant={row.get('dominant_phase', '-')}",
            f"catalog_messages={row.get('catalog_message_count', 0)}",
            f"media_worker_us={row.get('media_worker_us', 0)}",
            f"preview_backlog={row.get('preview_backlog', 0)}",
            f"preview_drained={row.get('preview_worker_drained', 0)}",
            f"status_due={row.get('status_write_due', 0)}",
            f"analytics_mode={row.get('analytics_mode', '-')}",
            f"dirty_y={row.get('dirty_y0', 0)}-{row.get('dirty_y1', 0)}",
        ]
        print("  " + " ".join(fields))


def run_self_test() -> int:
    header = [
        "frame",
        "elapsed_us",
        "selected",
        "visual_index",
        "home_screen",
        "arcade_update",
        "rows",
        "prepare_us",
        "slint_render_us",
        "custom_draw_us",
        "fb_present_us",
        "hidden_compose_us",
        "hidden_preview_compose_us",
        "hidden_arcade_compose_us",
        "direct_preview_present_us",
        "arcade_list_present_us",
        "runtime_status_write_us",
        "preview_apply_us",
        "preview_cache_inserts",
        "preview_cache_evictions",
        "vsync_us",
        "wall_us",
    ]
    rows = [
        [
            1,
            0,
            0,
            0,
            "arcade",
            "none",
            0,
            1000,
            1000,
            1000,
            1000,
            0,
            0,
            0,
            0,
            0,
            0,
            0,
            0,
            0,
            12000,
            16000,
        ],
        [
            2,
            16667,
            1,
            1,
            "arcade",
            "scroll",
            8,
            1000,
            1000,
            1000,
            1000,
            0,
            0,
            0,
            0,
            0,
            0,
            0,
            0,
            0,
            14000,
            17001,
        ],
        [
            3,
            33334,
            2,
            2,
            "arcade",
            "scroll",
            8,
            2000,
            1000,
            1000,
            2000,
            2300,
            1500,
            800,
            1500,
            800,
            0,
            900,
            1,
            1,
            16000,
            21000,
        ],
        [
            4,
            50001,
            3,
            3,
            "arcade",
            "scroll",
            8,
            18000,
            1000,
            1000,
            1000,
            0,
            0,
            0,
            0,
            0,
            0,
            0,
            0,
            0,
            2000,
            22000,
        ],
    ]
    parsed = [
        {key: parse_value(key, str(value)) for key, value in zip(header, row)}
        for row in rows
    ]
    for row in parsed:
        normalize_row(row)
    assert sum(1 for row in parsed if int(row["wall_us"]) > 16_667) == 3
    assert sum(1 for row in parsed if int(row["wall_us"]) > 18_000) == 2
    assert sum(1 for row in parsed if int(row["wall_us"]) > 20_000) == 2
    assert sum(1 for row in parsed if work_us(row) > 16_667) == 1
    assert (
        sum(
            1
            for row in parsed
            if int(row["wall_us"]) > 16_667 and work_us(row) <= 16_667
        )
        == 2
    )
    assert parsed[2]["hidden_compose_us"] == 2300
    assert parsed[2]["home_screen"] == "arcade"
    assert (
        parsed[2]["hidden_preview_compose_us"] + parsed[2]["hidden_arcade_compose_us"]
        == 2300
    )
    with tempfile.TemporaryDirectory() as tmp:
        path = Path(tmp) / "trace.tsv"
        with path.open("w", newline="") as f:
            writer = csv.writer(f, delimiter="\t")
            writer.writerow(header)
            writer.writerows(rows)
        with path.open(newline="") as f:
            reloaded = [
                {key: parse_value(key, value) for key, value in row.items()}
                for row in csv.DictReader(f, delimiter="\t")
            ]
        for row in reloaded:
            normalize_row(row)
        assert len(reloaded) == 4
    print("analyze-arcade-frame-trace self-test ok")
    return 0


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("trace", type=Path, nargs="?")
    parser.add_argument("--self-test", action="store_true")
    parser.add_argument(
        "--include-first",
        action="store_true",
        help="include frame 0 instead of treating it as startup warmup",
    )
    parser.add_argument("--worst", type=int, default=8)
    parser.add_argument(
        "--status-json",
        type=Path,
        help="optional runtime status JSON with retained slow-frame tombstones",
    )
    args = parser.parse_args()

    if args.self_test:
        return run_self_test()
    if args.trace is None:
        parser.error("trace is required unless --self-test is used")

    with args.trace.open(newline="") as f:
        rows = [
            {key: parse_value(key, value) for key, value in row.items()}
            for row in csv.DictReader(f, delimiter="\t")
        ]
    for row in rows:
        normalize_row(row)
    if not args.include_first:
        rows = [row for row in rows if int(row["frame"]) != 0]

    print(f"{args.trace}: frames={len(rows)} include_first={args.include_first}")
    print_smaller_stutter_report(rows)
    print()
    print_window_report(rows)
    print()
    print_status_tombstones(status_slow_frames(args.status_json))
    print()
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
    for row in sorted(rows, key=lambda item: int(item["wall_us"]), reverse=True)[
        : args.worst
    ]:
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
            "direct_preview_present_us",
            "arcade_list_present_us",
            "present_bytes",
            "wasted_present_bytes",
            "wall_us",
        ]
        print("\t".join(f"{field}={row[field]}" for field in fields if field in row))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
