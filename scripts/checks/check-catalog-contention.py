#!/usr/bin/env python3
# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

"""Require drop-free launcher frames that overlap measured catalog CPU work."""

from __future__ import annotations

import csv
import math
import sys
import tempfile
from pathlib import Path


TRACE_COLUMNS = {
    "frame",
    "elapsed_us",
    "monotonic_us",
    "prepare_us",
    "slint_render_us",
    "custom_draw_us",
    "hidden_compose_us",
    "fb_present_us",
    "wall_us",
    "vsync_source",
    "vsync_miss_streak",
    "main_present_status",
    "main_present_backend",
    "vsync_period_us",
    "present_phase_us",
    "main_present_hidden_copy_us",
    "main_present_request_us",
    "main_present_wait_us",
    "status_write_due",
    "runtime_status_write_us",
    "runtime_status_publish_sequence",
    "runtime_status_published_age_ms",
    "runtime_status_failures",
    "runtime_status_disconnected",
}
THREAD_COLUMNS = {
    "interval_start_monotonic_us",
    "monotonic_us",
    "thread_name",
    "utime_delta_jiffies",
    "stime_delta_jiffies",
}


def number(row: dict[str, str], key: str) -> int:
    try:
        return int(float(row.get(key, "") or 0))
    except ValueError:
        return 0


def percentile(values: list[int], pct: int) -> int:
    if not values:
        return 0
    ordered = sorted(values)
    index = max(0, min(len(ordered) - 1, math.ceil(len(ordered) * pct / 100) - 1))
    return ordered[index]


def emit(label: str, valid: bool, reason: str, **values: object) -> int:
    fields = "\t".join(f"{key}={value}" for key, value in values.items())
    print(
        f"catalog_contention_gate_tsv\tlabel={label}\tvalid={int(valid)}"
        f"\tinvalid_reason={reason}\t{fields}"
    )
    return 0 if valid else 9


def read_tsv(path: Path) -> list[dict[str, str]]:
    with path.open(newline="") as source:
        return list(csv.DictReader(source, delimiter="\t"))


def check(
    label: str,
    trace_path: Path,
    thread_path: Path,
    minimum_frames: int = 600,
    minimum_intervals: int = 10,
) -> int:
    try:
        frames = read_tsv(trace_path)
        threads = read_tsv(thread_path)
    except FileNotFoundError as error:
        return emit(label, False, "missing_input", detail=error.filename)
    if not frames or not threads:
        return emit(label, False, "empty_input", frames=len(frames), thread_rows=len(threads))
    missing_trace = sorted(TRACE_COLUMNS - set(frames[0]))
    missing_thread = sorted(THREAD_COLUMNS - set(threads[0]))
    if missing_trace or missing_thread:
        return emit(
            label,
            False,
            "missing_column",
            trace=",".join(missing_trace) or "ok",
            thread=",".join(missing_thread) or "ok",
        )

    active = []
    cpu_jiffies = 0
    for row in threads:
        name = row.get("thread_name", "")
        delta = number(row, "utime_delta_jiffies") + number(row, "stime_delta_jiffies")
        start = number(row, "interval_start_monotonic_us")
        end = number(row, "monotonic_us")
        if (name == "library-catalog" or name.startswith("catalog-v3")) and delta > 0 and end > start:
            active.append((start, end))
            cpu_jiffies += delta

    overlapping = [
        row
        for row in frames
        if any(start <= number(row, "monotonic_us") <= end for start, end in active)
    ]
    common = {
        "overlap_frames": len(overlapping),
        "active_intervals": len(active),
        "catalog_cpu_jiffies": cpu_jiffies,
        "minimum_frames": minimum_frames,
        "minimum_intervals": minimum_intervals,
    }
    if len(active) < minimum_intervals or len(overlapping) < minimum_frames:
        return emit(label, False, "insufficient_overlap", **common)

    bad_work = 0
    bad_wall = 0
    bad_vsync = 0
    bad_present = 0
    bad_backend = 0
    bad_deadline = 0
    work_samples = []
    for row in frames:
        work = sum(
            number(row, key)
            for key in (
                "prepare_us",
                "slint_render_us",
                "custom_draw_us",
                "hidden_compose_us",
                "fb_present_us",
            )
        )
        work_samples.append(work)
        bad_work += int(work > 16_667)
        bad_wall += int(number(row, "wall_us") > 33_334)
        bad_vsync += int(
            row.get("vsync_source") != "vsync" or number(row, "vsync_miss_streak") != 0
        )
        bad_present += int(row.get("main_present_status") != "ok")
        bad_backend += int(row.get("main_present_backend") != "fpga-vblank-latch-hidden")
        latch_done = sum(
            number(row, key)
            for key in (
                "present_phase_us",
                "main_present_hidden_copy_us",
                "main_present_request_us",
                "main_present_wait_us",
            )
        )
        bad_deadline += int(latch_done > (number(row, "vsync_period_us") or 16_667))

    published = [row for row in frames if number(row, "runtime_status_publish_sequence") > 0]
    status_due = [row for row in frames if number(row, "status_write_due") > 0]
    status_sequence = max(
        (number(row, "runtime_status_publish_sequence") for row in frames), default=0
    )
    first_status_publish_ms = min(
        (number(row, "elapsed_us") // 1_000 for row in published), default=2**63 - 1
    )
    status_age_max = max(
        (number(row, "runtime_status_published_age_ms") for row in published), default=0
    )
    status_failures = max(
        (number(row, "runtime_status_failures") for row in frames), default=0
    )
    status_disconnected = sum(
        number(row, "runtime_status_disconnected") > 0 for row in frames
    )
    status_enqueue_p99 = percentile(
        [number(row, "runtime_status_write_us") for row in status_due], 99
    )
    work_p99 = percentile(work_samples, 99)
    counts = {
        **common,
        "work_over_budget": bad_work,
        "wall_over_two_frames": bad_wall,
        "vsync_drop_frames": bad_vsync,
        "present_failures": bad_present,
        "non_latch_frames": bad_backend,
        "latch_deadline_misses": bad_deadline,
        "work_p99_us": work_p99,
        "work_p99_budget_us": 6_809,
        "runtime_status_sequence": status_sequence,
        "runtime_status_first_publish_ms": first_status_publish_ms,
        "runtime_status_age_max_ms": status_age_max,
        "runtime_status_age_budget_ms": 5_000,
        "runtime_status_failures": status_failures,
        "runtime_status_disconnected_frames": status_disconnected,
        "runtime_status_enqueue_p99_us": status_enqueue_p99,
        "runtime_status_enqueue_budget_us": 500,
    }
    status_valid = (
        status_sequence > 0
        and bool(status_due)
        and first_status_publish_ms <= 5_000
        and status_age_max <= 5_000
        and status_failures == 0
        and status_disconnected == 0
        and status_enqueue_p99 <= 500
    )
    valid = (
        not any((bad_work, bad_wall, bad_vsync, bad_present, bad_backend, bad_deadline))
        and work_p99 <= 6_809
        and status_valid
    )
    reason = (
        "ok"
        if valid
        else "runtime_status"
        if not status_valid
        else "work_p99"
        if work_p99 > 6_809
        else "frame_drop"
    )
    return emit(label, valid, reason, **counts)


def self_test() -> int:
    with tempfile.TemporaryDirectory() as directory:
        root = Path(directory)
        trace = root / "trace.tsv"
        thread = root / "threads.tsv"
        trace.write_text(
            "frame\telapsed_us\tmonotonic_us\tprepare_us\tslint_render_us\tcustom_draw_us\thidden_compose_us\tfb_present_us\twall_us\tvsync_source\tvsync_miss_streak\tmain_present_status\tmain_present_backend\tvsync_period_us\tpresent_phase_us\tmain_present_hidden_copy_us\tmain_present_request_us\tmain_present_wait_us\tstatus_write_due\truntime_status_write_us\truntime_status_publish_sequence\truntime_status_published_age_ms\truntime_status_failures\truntime_status_disconnected\n"
            + "".join(
                f"{index}\t{index * 1000}\t{index * 1000}\t100\t100\t100\t100\t100\t16667\tvsync\t0\tok\tfpga-vblank-latch-hidden\t16667\t1000\t1200\t20\t10\t1\t100\t{index}\t50\t0\t0\n"
                for index in range(1, 21)
            )
        )
        thread.write_text(
            "thread_sample_tsv\tinterval_start_monotonic_us\tmonotonic_us\tthread_name\tutime_delta_jiffies\tstime_delta_jiffies\n"
            + "".join(
                f"thread_sample_tsv\t{index * 1000 - 999}\t{index * 1000}\tlibrary-catalog\t1\t0\n"
                for index in range(1, 21)
            )
        )
        if check("self-pass", trace, thread, 20, 20) != 0:
            return 1
        good_trace = trace.read_text()
        broken = good_trace.replace(
            "\tvsync\t0\tok\t", "\tfallback\t1\tok\t", 1
        )
        trace.write_text(broken)
        if check("self-drop", trace, thread, 20, 20) == 0:
            print("self-test accepted a dropped frame", file=sys.stderr)
            return 1
        trace.write_text(good_trace.replace("\t50\t0\t0\n", "\t5001\t0\t0\n"))
        if check("self-stale-status", trace, thread, 20, 20) == 0:
            print("self-test accepted stale runtime status", file=sys.stderr)
            return 1
        thread.write_text(thread.read_text().replace("\t1\t0\n", "\t0\t0\n"))
        if check("self-quiet", trace, thread, 20, 20) == 0:
            print("self-test accepted a quiet catalog", file=sys.stderr)
            return 1
    print("catalog contention checker self-test ok")
    return 0


def main(argv: list[str]) -> int:
    if argv[1:] == ["--self-test"]:
        return self_test()
    if len(argv) not in (4, 5, 6):
        print(
            "usage: check-catalog-contention.py LABEL TRACE THREAD_SAMPLE [MINIMUM_FRAMES] [MINIMUM_INTERVALS]",
            file=sys.stderr,
        )
        return 2
    return check(
        argv[1],
        Path(argv[2]),
        Path(argv[3]),
        int(argv[4]) if len(argv) >= 5 else 600,
        int(argv[5]) if len(argv) == 6 else 10,
    )


if __name__ == "__main__":
    sys.exit(main(sys.argv))
