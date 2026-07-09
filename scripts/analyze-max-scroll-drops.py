#!/usr/bin/env python3
"""Summarize max-speed launcher scroll drops from a frame trace."""

from __future__ import annotations

import argparse
import csv
import json
import math
import sys
from collections import Counter
from dataclasses import dataclass
from pathlib import Path


FRAME_BUDGET_US = 16_667
LATCH_BACKEND = "fpga-vblank-latch-hidden"
FPGA_COUNTER_MODULUS = 65_536
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
    "frame_finish_us",
    "post_finish_tail_us",
    "vsync_source",
    "vsync_miss_streak",
    "vsync_wait_start_age_us",
    "vsync_accepted_hit_age_us",
    "frame_start_phase_us",
    "present_phase_us",
    "main_present_backend",
    "main_present_status",
    "main_present_buffer",
    "main_present_route_us",
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


def read_rows(
    path: Path, ignore_frames_through: int, ignore_elapsed_zero: bool
) -> list[dict[str, str]]:
    with path.open(newline="", encoding="utf-8") as f:
        rows = list(csv.DictReader(f, delimiter="\t"))
    return [
        row
        for row in rows
        if int_field(row, "frame") > ignore_frames_through
        and (not ignore_elapsed_zero or int_field(row, "elapsed_us") != 0)
    ]


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


@dataclass(frozen=True)
class FpgaLatchReport:
    supported: bool
    flip_count: int
    post_count: int
    drop_count: int


def parse_key_value_tsv(line: str) -> dict[str, str]:
    values: dict[str, str] = {}
    for part in line.rstrip().split("\t")[1:]:
        if "=" in part:
            key, value = part.split("=", 1)
            values[key] = value
    return values


def fpga_latch_report(path: Path | None) -> FpgaLatchReport | None:
    if path is None or not path.exists() or path.stat().st_size == 0:
        return None
    for line in path.read_text(encoding="utf-8", errors="replace").splitlines():
        if not line.startswith("fpga_latch_status_tsv\t"):
            continue
        fields = parse_key_value_tsv(line)
        return FpgaLatchReport(
            supported=fields.get("supported") == "1",
            flip_count=int(fields.get("flip_count", "0") or 0),
            post_count=int(fields.get("post_count", "0") or 0),
            drop_count=int(fields.get("drop_count", "0") or 0),
        )
    return None


def counter_delta(before: int, after: int) -> int:
    return (after - before) % FPGA_COUNTER_MODULUS


def buffer_alternation_failures(rows: list[dict[str, str]]) -> int:
    failures = 0
    previous: int | None = None
    for row in rows:
        buffer_index = int_field(row, "main_present_buffer")
        if buffer_index not in (1, 2):
            failures += 1
        elif previous is not None and buffer_index == previous:
            failures += 1
        previous = buffer_index
    return failures


def flip_counter_sample_failures(rows: list[dict[str, str]]) -> tuple[int, int, int]:
    previous_value: int | None = None
    previous_index: int | None = None
    samples = 0
    failures = 0
    observed = 0
    baseline_established = False
    for index, row in enumerate(rows):
        value = int_field(row, "main_present_route_us")
        if value <= 0:
            continue
        if previous_value is None:
            previous_value = value
            previous_index = index
            continue
        if value == previous_value:
            continue
        observed = 1
        if not baseline_established:
            previous_value = value
            previous_index = index
            baseline_established = True
            continue
        samples += 1
        row_delta = index - (previous_index or 0)
        value_delta = counter_delta(previous_value, value)
        if value_delta != row_delta:
            failures += 1
        previous_value = value
        previous_index = index
    return observed, samples, failures


def print_summary(
    label: str,
    rows: list[dict[str, str]],
    slow_frames: list[dict[str, object]],
    worst_count: int,
    expect_backend: str | None,
    fpga_before: FpgaLatchReport | None = None,
    fpga_after: FpgaLatchReport | None = None,
) -> int:
    if not rows:
        print(
            f"max_scroll_gate_tsv\tlabel={label}\tvalid=0\tinvalid_reason=no_measured_frames"
        )
        return 9

    walls = [int_field(row, "wall_us") for row in rows]
    works = [work_us(row) for row in rows]
    over = [row for row in rows if int_field(row, "wall_us") > FRAME_BUDGET_US]
    loop_over = [row for row in rows if int_field(row, "loop_delta_us") > FRAME_BUDGET_US]
    cadence_misses = [
        row
        for row in rows
        if int_field(row, "wall_us") > FRAME_BUDGET_US
        or int_field(row, "loop_delta_us") > FRAME_BUDGET_US
    ]
    near = [row for row in rows if int_field(row, "wall_us") >= 16_000]
    latch_rows = [row for row in rows if is_latch_row(row)]
    latch_misses = [row for row in latch_rows if latch_deadline_margin_us(row) < 0]
    latch_margins = [latch_deadline_margin_us(row) for row in latch_rows]
    latch_copy = [int_field(row, "main_present_hidden_copy_us") for row in latch_rows]
    latch_post = [int_field(row, "main_present_request_us") for row in latch_rows]
    latch_status = [int_field(row, "main_present_wait_us") for row in latch_rows]
    frame_finish = [int_field(row, "frame_finish_us") for row in rows]
    post_finish_tail = [int_field(row, "post_finish_tail_us") for row in rows]
    latch_miss_frame_finish = [int_field(row, "frame_finish_us") for row in latch_misses]
    latch_miss_post_finish_tail = [
        int_field(row, "post_finish_tail_us") for row in latch_misses
    ]
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
    buffer_failures = buffer_alternation_failures(latch_rows)
    flip_observed, flip_samples, flip_failures = flip_counter_sample_failures(latch_rows)
    fpga_report_required = expect_backend == LATCH_BACKEND
    fpga_report_present = fpga_after is not None
    fpga_report_supported = fpga_after.supported if fpga_after is not None else False
    fpga_drop_count_max = max(
        [report.drop_count for report in (fpga_before, fpga_after) if report is not None],
        default=0,
    )
    fpga_flip_delta = (
        counter_delta(fpga_before.flip_count, fpga_after.flip_count)
        if fpga_before is not None and fpga_after is not None
        else 0
    )
    fpga_post_delta = (
        counter_delta(fpga_before.post_count, fpga_after.post_count)
        if fpga_before is not None and fpga_after is not None
        else 0
    )
    fpga_counters_advanced = (
        fpga_before is None
        or fpga_after is None
        or (fpga_flip_delta > 0 and fpga_post_delta > 0)
    )
    fpga_report_valid = (
        (not fpga_report_required or fpga_report_present)
        and (not fpga_report_present or fpga_report_supported)
        and fpga_drop_count_max == 0
        and fpga_counters_advanced
    )
    visual_latch_misses = len(latch_misses) + buffer_failures + flip_failures
    scheduler_wake_jitter_misses = len(cadence_misses)
    latch_valid = not latch_rows or (
        len(latch_misses) == 0
        and buffer_failures == 0
        and flip_failures == 0
        and status_counts == Counter({"ok": len(rows)})
        and fpga_report_valid
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
        f"loop_drops_gt_16667={len(loop_over)} "
        f"strict_cadence_misses={len(cadence_misses)} "
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
        f"visual_latch_misses={visual_latch_misses} "
        f"scheduler_wake_jitter_misses={scheduler_wake_jitter_misses} "
        f"buffer_alternation_failures={buffer_failures} "
        f"flip_counter_observed={flip_observed} "
        f"flip_counter_samples={flip_samples} "
        f"flip_counter_gaps={flip_failures} "
        f"fpga_latch_report_present={1 if fpga_report_present else 0} "
        f"fpga_latch_report_supported={1 if fpga_report_supported else 0} "
        f"fpga_drop_count_max={fpga_drop_count_max} "
        f"fpga_flip_delta={fpga_flip_delta} "
        f"fpga_post_delta={fpga_post_delta} "
        f"fpga_counters_advanced={1 if fpga_counters_advanced else 0} "
        f"latch_margin_p50={percentile(latch_margins, 50)} "
        f"latch_margin_p95={percentile(latch_margins, 95)} "
        f"latch_margin_p99={percentile(latch_margins, 99)} "
        f"latch_margin_min={min(latch_margins) if latch_margins else 0} "
        f"latch_copy_p50={percentile(latch_copy, 50)} "
        f"latch_copy_p95={percentile(latch_copy, 95)} "
        f"latch_copy_p99={percentile(latch_copy, 99)} "
        f"latch_post_p99={percentile(latch_post, 99)} "
        f"latch_status_p99={percentile(latch_status, 99)} "
        f"frame_finish_p99={percentile(frame_finish, 99)} "
        f"post_finish_tail_p99={percentile(post_finish_tail, 99)} "
        f"latch_miss_frame_finish_p50={percentile(latch_miss_frame_finish, 50)} "
        f"latch_miss_post_finish_tail_p50={percentile(latch_miss_post_finish_tail, 50)} "
        f"dominant_over_budget={dict(phase_counts.most_common())}"
    )
    if expect_backend == LATCH_BACKEND or latch_rows:
        valid = latch_valid
        if valid:
            invalid_reason = "ok"
        else:
            missed_deadline = len(latch_misses) > 0 or not backend_valid
            missed_cadence = len(cadence_misses) > 0
            invalid_reason = (
                "latch_visual_and_scheduler"
                if missed_deadline and missed_cadence
                else "scheduler_wake_jitter"
                if missed_cadence and visual_latch_misses == 0 and backend_valid and fpga_report_valid
                else "latch_visual_or_backend"
            )
    else:
        valid = len(cadence_misses) == 0 and backend_valid
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
            "loop_delta_us": int_field(row, "loop_delta_us"),
            "work_us": work_us(row),
            "over_budget_us": max(0, int_field(row, "wall_us") - FRAME_BUDGET_US),
            "loop_over_budget_us": max(
                0, int_field(row, "loop_delta_us") - FRAME_BUDGET_US
            ),
            "latch_post_done_phase_us": latch_post_done_phase_us(row),
            "latch_deadline_margin_us": latch_deadline_margin_us(row),
            "deadline_miss": int(latch_deadline_margin_us(row) < 0),
            "cadence_miss": int(
                int_field(row, "wall_us") > FRAME_BUDGET_US
                or int_field(row, "loop_delta_us") > FRAME_BUDGET_US
            ),
            "scheduler_wake_jitter_miss": int(
                int_field(row, "wall_us") > FRAME_BUDGET_US
                or int_field(row, "loop_delta_us") > FRAME_BUDGET_US
            ),
            "visual_latch_miss": int(latch_deadline_margin_us(row) < 0),
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
        "loop_delta_us": "16300",
        "vsync_period_us": "16667",
        "present_phase_us": "1000",
        "main_present_backend": LATCH_BACKEND,
        "main_present_status": "ok",
        "main_present_hidden_copy_us": "1200",
        "main_present_request_us": "20",
        "main_present_wait_us": "10",
        "main_present_buffer": "1",
        "main_present_route_us": "100",
    }
    next_frame = dict(base)
    next_frame["frame"] = "32"
    next_frame["main_present_buffer"] = "2"
    next_frame["main_present_route_us"] = "101"
    report_before = FpgaLatchReport(True, 100, 100, 0)
    report_after = FpgaLatchReport(True, 102, 102, 0)
    missed = dict(base)
    missed["present_phase_us"] = "16000"
    cadence_missed = dict(base)
    cadence_missed["wall_us"] = "17000"
    cadence_missed["loop_delta_us"] = "17000"
    cadence_next = dict(cadence_missed)
    cadence_next["frame"] = "32"
    cadence_next["main_present_buffer"] = "2"
    cadence_next["main_present_route_us"] = "101"
    repeated_buffer = dict(next_frame)
    repeated_buffer["main_present_buffer"] = "1"
    flip_gap = dict(next_frame)
    flip_gap["main_present_route_us"] = "103"
    flip_gap_later = dict(base)
    flip_gap_later["frame"] = "33"
    flip_gap_later["main_present_buffer"] = "1"
    flip_gap_later["main_present_route_us"] = "106"
    if latch_deadline_margin_us(base) <= 0:
        print("self-test expected positive latch margin", file=sys.stderr)
        return 1
    if latch_deadline_margin_us(missed) >= 0:
        print("self-test expected negative latch margin", file=sys.stderr)
        return 1
    if print_summary("self-latch-pass", [base, next_frame], [], 1, LATCH_BACKEND, report_before, report_after) != 0:
        print("self-test expected latch pass", file=sys.stderr)
        return 1
    if print_summary("self-latch-fail", [missed, next_frame], [], 1, LATCH_BACKEND, report_before, report_after) == 0:
        print("self-test expected latch failure", file=sys.stderr)
        return 1
    if print_summary("self-latch-jitter-pass", [cadence_missed, cadence_next], [], 1, LATCH_BACKEND, report_before, report_after) != 0:
        print("self-test expected latch scheduler-jitter pass", file=sys.stderr)
        return 1
    if print_summary("self-latch-buffer-fail", [base, repeated_buffer], [], 1, LATCH_BACKEND, report_before, report_after) == 0:
        print("self-test expected latch buffer failure", file=sys.stderr)
        return 1
    if print_summary("self-latch-flip-gap-fail", [base, flip_gap, flip_gap_later], [], 1, LATCH_BACKEND, report_before, report_after) == 0:
        print("self-test expected latch flip gap failure", file=sys.stderr)
        return 1
    dropped_report = FpgaLatchReport(True, 102, 102, 1)
    if print_summary("self-latch-fpga-drop-fail", [base, next_frame], [], 1, LATCH_BACKEND, report_before, dropped_report) == 0:
        print("self-test expected latch FPGA drop failure", file=sys.stderr)
        return 1
    deadline_only = dict(missed)
    deadline_only["wall_us"] = "16300"
    deadline_only["loop_delta_us"] = "16300"
    if print_summary("self-latch-deadline-only-fail", [deadline_only, next_frame], [], 1, LATCH_BACKEND, report_before, report_after) == 0:
        print("self-test expected latch deadline-only failure", file=sys.stderr)
        return 1
    return 0


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("trace", type=Path, nargs="?")
    parser.add_argument("--label", default="max-scroll")
    parser.add_argument("--status-json", type=Path)
    parser.add_argument("--fpga-latch-report-before", type=Path)
    parser.add_argument("--fpga-latch-report-after", type=Path)
    parser.add_argument("--ignore-frames-through", type=int, default=30)
    parser.add_argument("--ignore-elapsed-zero", action="store_true")
    parser.add_argument("--worst", type=int, default=12)
    parser.add_argument("--expect-backend", choices=[LATCH_BACKEND, "fb0-dirty"])
    parser.add_argument("--self-test", action="store_true")
    args = parser.parse_args()

    if args.self_test:
        return run_self_test()
    if args.trace is None:
        parser.error("trace is required unless --self-test is used")

    rows = read_rows(args.trace, args.ignore_frames_through, args.ignore_elapsed_zero)
    return print_summary(
        args.label,
        rows,
        status_slow_frames(args.status_json),
        args.worst,
        args.expect_backend,
        fpga_latch_report(args.fpga_latch_report_before),
        fpga_latch_report(args.fpga_latch_report_after),
    )


if __name__ == "__main__":
    raise SystemExit(main())
