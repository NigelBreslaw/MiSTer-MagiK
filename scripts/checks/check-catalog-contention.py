#!/usr/bin/env python3
# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

"""Require drop-free launcher frames that overlap measured catalog CPU work."""

from __future__ import annotations

import csv
import sys
import tempfile
from pathlib import Path


TRACE_COLUMNS = {
    "frame",
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
    for row in overlapping:
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
        bad_work += int(work > 16_667)
        bad_wall += int(number(row, "wall_us") > 33_334)
        bad_vsync += int(
            row.get("vsync_source") != "vsync" or number(row, "vsync_miss_streak") != 0
        )
        bad_present += int(row.get("main_present_status") != "ok")
    counts = {
        **common,
        "work_over_budget": bad_work,
        "wall_over_two_frames": bad_wall,
        "vsync_drop_frames": bad_vsync,
        "present_failures": bad_present,
    }
    valid = not any((bad_work, bad_wall, bad_vsync, bad_present))
    return emit(label, valid, "ok" if valid else "frame_drop", **counts)


def self_test() -> int:
    with tempfile.TemporaryDirectory() as directory:
        root = Path(directory)
        trace = root / "trace.tsv"
        thread = root / "threads.tsv"
        trace.write_text(
            "frame\tmonotonic_us\tprepare_us\tslint_render_us\tcustom_draw_us\thidden_compose_us\tfb_present_us\twall_us\tvsync_source\tvsync_miss_streak\tmain_present_status\n"
            + "".join(
                f"{index}\t{index * 1000}\t100\t100\t100\t100\t100\t16667\tvsync\t0\tok\n"
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
        broken = trace.read_text().replace("\tvsync\t0\tok\n", "\tfallback\t1\tok\n", 1)
        trace.write_text(broken)
        if check("self-drop", trace, thread, 20, 20) == 0:
            print("self-test accepted a dropped frame", file=sys.stderr)
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
