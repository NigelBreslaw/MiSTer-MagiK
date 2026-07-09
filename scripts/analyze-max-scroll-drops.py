#!/usr/bin/env python3
"""Summarize max-speed launcher scroll drops from a frame trace."""

from __future__ import annotations

import argparse
import csv
import json
import math
import sys
from collections import Counter
from pathlib import Path


FRAME_BUDGET_US = 16_667
LATCH_BACKEND = "fpga-vblank-latch-hidden"
PHASE_COLUMNS = [
    "prepare_us",
    "slint_render_us",
    "custom_draw_us",
    "vsync_us",
    "fb_present_us",
    "cached_present_us",
    "direct_preview_present_us",
    "arcade_list_present_us",
    "preview_apply_us",
    "runtime_status_write_us",
]
CONTEXT_COLUMNS = [
    "catalog_worker_us",
    "catalog_message_count",
    "catalog_backlog",
    "media_worker_us",
    "media_gate_us",
    "preview_schedule_us",
    "preview_apply_us",
    "preview_backlog",
    "preview_worker_drained",
    "preview_cache_inserts",
    "preview_cache_evictions",
    "status_write_due",
    "runtime_status_write_us",
    "vsync_source",
    "vsync_miss_streak",
    "vsync_wait_start_age_us",
    "vsync_accepted_hit_age_us",
    "frame_start_phase_us",
    "present_phase_us",
    "main_present_backend",
    "main_present_status",
]


def int_field(row: dict[str, str], key: str) -> int:
    try:
        return int(float(row.get(key, "") or 0))
    except ValueError:
        return 0


def percentile(values: list[int], pct: int) -> int:
    if not values:
        return 0
    values = sorted(values)
    index = max(0, min(len(values) - 1, math.ceil(len(values) * pct / 100) - 1))
    return values[index]


def work_us(row: dict[str, str]) -> int:
    return (
        int_field(row, "prepare_us")
        + int_field(row, "slint_render_us")
        + int_field(row, "custom_draw_us")
        + int_field(row, "fb_present_us")
    )


def latch_post_done_phase_us(row: dict[str, str]) -> int:
    return (
        int_field(row, "present_phase_us")
        + int_field(row, "main_present_hidden_copy_us")
        + int_field(row, "main_present_request_us")
        + int_field(row, "main_present_wait_us")
    )


def latch_deadline_margin_us(row: dict[str, str]) -> int:
    period = int_field(row, "vsync_period_us") or FRAME_BUDGET_US
    return period - latch_post_done_phase_us(row)


def is_latch_row(row: dict[str, str]) -> bool:
    return row.get("main_present_backend", "") == LATCH_BACKEND


def dominant_phase(row: dict[str, str], medians: dict[str, int]) -> str:
    best_column = "none"
    best_delta = 0
    for column in PHASE_COLUMNS:
        if column not in row:
            continue
        delta = int_field(row, column) - medians.get(column, 0)
        if delta > best_delta:
            best_column = column
            best_delta = delta
    return best_column


def read_rows(path: Path, ignore_frames_through: int) -> list[dict[str, str]]:
    with path.open(newline="", encoding="utf-8") as f:
        rows = list(csv.DictReader(f, delimiter="\t"))
    return [row for row in rows if int_field(row, "frame") > ignore_frames_through]


def status_slow_frames(path: Path | None) -> list[dict[str, object]]:
    if path is None or not path.exists() or path.stat().st_size == 0:
        return []
    with path.open(encoding="utf-8") as f:
        data = json.load(f)
    candidates = [
        data.get("runtime", {}).get("slint_status", {}),
        data.get("launcher", {}),
        data,
    ]
    for candidate in candidates:
        if not isinstance(candidate, dict):
            continue
        frame_budget = candidate.get("frame_budget", {})
        if not isinstance(frame_budget, dict):
            continue
        slow = frame_budget.get("slow_frames", [])
        if isinstance(slow, list):
            return [row for row in slow if isinstance(row, dict)]
    return []


def print_summary(
    label: str,
    rows: list[dict[str, str]],
    slow_frames: list[dict[str, object]],
    worst_count: int,
    expect_backend: str | None,
) -> int:
    if not rows:
        print(
            f"max_scroll_gate_tsv\tlabel={label}\tvalid=0\tinvalid_reason=no_measured_frames"
        )
        return 9

    walls = [int_field(row, "wall_us") for row in rows]
    works = [work_us(row) for row in rows]
    over = [row for row in rows if int_field(row, "wall_us") > FRAME_BUDGET_US]
    near = [row for row in rows if int_field(row, "wall_us") >= 16_000]
    latch_rows = [row for row in rows if is_latch_row(row)]
    latch_misses = [row for row in latch_rows if latch_deadline_margin_us(row) < 0]
    latch_margins = [latch_deadline_margin_us(row) for row in latch_rows]
    latch_copy = [int_field(row, "main_present_hidden_copy_us") for row in latch_rows]
    latch_post = [int_field(row, "main_present_request_us") for row in latch_rows]
    latch_status = [int_field(row, "main_present_wait_us") for row in latch_rows]
    medians = {
        column: percentile([int_field(row, column) for row in rows], 50)
        for column in PHASE_COLUMNS
        if column in rows[0]
    }
    phase_counts = Counter(dominant_phase(row, medians) for row in over)
    source_counts = Counter(row.get("vsync_source", "") or "blank" for row in rows)
    backend_counts = Counter(row.get("main_present_backend", "") or "blank" for row in rows)
    status_counts = Counter(row.get("main_present_status", "") or "blank" for row in rows)
    backend_valid = expect_backend is None or backend_counts == Counter({expect_backend: len(rows)})
    latch_valid = not latch_rows or (
        len(latch_misses) == 0
        and status_counts == Counter({"ok": len(rows)})
        and backend_valid
    )
    over_work = sum(1 for row in rows if work_us(row) > FRAME_BUDGET_US)
    low_work_high_wall = sum(
        1
        for row in rows
        if int_field(row, "wall_us") > FRAME_BUDGET_US and work_us(row) <= FRAME_BUDGET_US
    )
    detail = (
        f"frames={len(rows)} near_ge_16000={len(near)} drops_gt_16667={len(over)} "
        f"wall_p50={percentile(walls, 50)} wall_p95={percentile(walls, 95)} "
        f"wall_p99={percentile(walls, 99)} wall_max={max(walls)} "
        f"work_p99={percentile(works, 99)} work_max={max(works)} "
        f"work_gt_16667={over_work} low_work_high_wall={low_work_high_wall} "
        f"wall_gt_18000={sum(1 for value in walls if value > 18_000)} "
        f"wall_gt_20000={sum(1 for value in walls if value > 20_000)} "
        f"wall_gt_33334={sum(1 for value in walls if value > 33_334)} "
        f"vsync_sources={dict(sorted(source_counts.items()))} "
        f"present_backends={dict(sorted(backend_counts.items()))} "
        f"present_status={dict(sorted(status_counts.items()))} "
        f"expect_backend={expect_backend or ''} "
        f"backend_valid={1 if backend_valid else 0} "
        f"latch_frames={len(latch_rows)} "
        f"latch_deadline_misses={len(latch_misses)} "
        f"latch_margin_p50={percentile(latch_margins, 50)} "
        f"latch_margin_p95={percentile(latch_margins, 95)} "
        f"latch_margin_p99={percentile(latch_margins, 99)} "
        f"latch_margin_min={min(latch_margins) if latch_margins else 0} "
        f"latch_copy_p50={percentile(latch_copy, 50)} "
        f"latch_copy_p95={percentile(latch_copy, 95)} "
        f"latch_copy_p99={percentile(latch_copy, 99)} "
        f"latch_post_p99={percentile(latch_post, 99)} "
        f"latch_status_p99={percentile(latch_status, 99)} "
        f"dominant_over_budget={dict(phase_counts.most_common())}"
    )
    if expect_backend == LATCH_BACKEND or latch_rows:
        valid = latch_valid
        invalid_reason = "ok" if valid else "latch_deadline_or_backend"
    else:
        valid = len(over) == 0 and backend_valid
        invalid_reason = "ok" if valid else "over_budget_frames"
    print(
        f"max_scroll_gate_tsv\tlabel={label}\tvalid={1 if valid else 0}"
        f"\tinvalid_reason={invalid_reason}\t{detail}"
    )

    print("max_scroll_worst_frames_tsv")
    for row in sorted(rows, key=lambda item: int_field(item, "wall_us"), reverse=True)[
        :worst_count
    ]:
        fields = {
            "frame": row.get("frame", ""),
            "elapsed_us": row.get("elapsed_us", ""),
            "selected": row.get("selected", ""),
            "visual_index": row.get("visual_index", ""),
            "wall_us": int_field(row, "wall_us"),
            "work_us": work_us(row),
            "over_budget_us": max(0, int_field(row, "wall_us") - FRAME_BUDGET_US),
            "latch_post_done_phase_us": latch_post_done_phase_us(row),
            "latch_deadline_margin_us": latch_deadline_margin_us(row),
            "dominant_delta": dominant_phase(row, medians),
        }
        for column in PHASE_COLUMNS + CONTEXT_COLUMNS:
            if column in row:
                fields[column] = row.get(column, "")
        print("\t".join(f"{key}={value}" for key, value in fields.items()))

    if slow_frames:
        print("max_scroll_runtime_slow_frames_tsv")
        for row in sorted(
            slow_frames, key=lambda item: int(item.get("wall_us", 0) or 0), reverse=True
        )[:worst_count]:
            fields = [
                "frame",
                "severity",
                "wall_us",
                "over_budget_us",
                "dominant_phase",
                "catalog_message_count",
                "catalog_backlog",
                "media_worker_us",
                "preview_apply_us",
                "preview_backlog",
                "preview_worker_drained",
                "runtime_status_write_us",
                "vsync_source",
                "vsync_miss_streak",
                "main_present_backend",
                "main_present_status",
            ]
            print(
                "\t".join(f"{field}={row.get(field, '')}" for field in fields if field in row)
            )
    return 0 if valid else 9


def run_self_test() -> int:
    base = {
        "frame": "31",
        "elapsed_us": "1000",
        "prepare_us": "100",
        "slint_render_us": "200",
        "custom_draw_us": "10",
        "vsync_us": "15000",
        "fb_present_us": "1300",
        "wall_us": "16300",
        "vsync_period_us": "16667",
        "present_phase_us": "1000",
        "main_present_backend": LATCH_BACKEND,
        "main_present_status": "ok",
        "main_present_hidden_copy_us": "1200",
        "main_present_request_us": "20",
        "main_present_wait_us": "10",
    }
    missed = dict(base)
    missed["present_phase_us"] = "16000"
    if latch_deadline_margin_us(base) <= 0:
        print("self-test expected positive latch margin", file=sys.stderr)
        return 1
    if latch_deadline_margin_us(missed) >= 0:
        print("self-test expected negative latch margin", file=sys.stderr)
        return 1
    if print_summary("self-latch-pass", [base], [], 1, LATCH_BACKEND) != 0:
        print("self-test expected latch pass", file=sys.stderr)
        return 1
    if print_summary("self-latch-fail", [missed], [], 1, LATCH_BACKEND) == 0:
        print("self-test expected latch failure", file=sys.stderr)
        return 1
    return 0


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("trace", type=Path, nargs="?")
    parser.add_argument("--label", default="max-scroll")
    parser.add_argument("--status-json", type=Path)
    parser.add_argument("--ignore-frames-through", type=int, default=30)
    parser.add_argument("--worst", type=int, default=12)
    parser.add_argument("--expect-backend", choices=[LATCH_BACKEND, "fb0-dirty"])
    parser.add_argument("--self-test", action="store_true")
    args = parser.parse_args()

    if args.self_test:
        return run_self_test()
    if args.trace is None:
        parser.error("trace is required unless --self-test is used")

    rows = read_rows(args.trace, args.ignore_frames_through)
    return print_summary(
        args.label,
        rows,
        status_slow_frames(args.status_json),
        args.worst,
        args.expect_backend,
    )


if __name__ == "__main__":
    raise SystemExit(main())
