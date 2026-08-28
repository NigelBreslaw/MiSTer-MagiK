#!/usr/bin/env python3
# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

"""Summarize and compare launcher present-path frame traces.

The trace is produced by MISTER_PREVIEW_SCROLL_TRACE from the real
Main-supervised launcher. This tool keeps RGB565 present/copy regressions out of
the generic whole-frame pacing bucket.
"""

from __future__ import annotations

import argparse
import csv
import math
import sys
import tempfile
from collections.abc import Iterable
from dataclasses import dataclass
from pathlib import Path

REQUIRED_COLUMNS = {
    "frame",
    "arcade_update",
    "rows",
    "fb_present_us",
    "cached_present_us",
    "arcade_list_present_us",
    "vsync_source",
    "vsync_miss_streak",
}

METRICS = [
    "rows",
    "direct_preview_rows",
    "present_bytes",
    "wasted_present_bytes",
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
]

FAILURE_EXIT = 9


@dataclass(frozen=True)
class MetricStats:
    avg: float
    p50: int
    p95: int
    p99: int
    max: int


@dataclass(frozen=True)
class TraceData:
    path: Path
    rows: list[dict[str, str]]


def int_field(row: dict[str, str], key: str) -> int:
    try:
        return int(float(row.get(key, "") or 0))
    except ValueError:
        return 0


def percent_delta(before: float, after: float) -> float:
    if before == 0:
        return 0.0 if after == 0 else math.inf
    return ((after - before) / before) * 100.0


def fmt_float(value: float, digits: int = 1) -> str:
    if math.isinf(value):
        return "inf"
    return f"{value:.{digits}f}"


def percentile(values: list[int], pct: int) -> int:
    if not values:
        return 0
    sorted_values = sorted(values)
    idx = math.ceil(len(sorted_values) * pct / 100.0) - 1
    return sorted_values[max(0, min(len(sorted_values) - 1, idx))]


def metric_stats(rows: list[dict[str, str]], metric: str) -> MetricStats:
    values = [int_field(row, metric) for row in rows]
    if not values:
        return MetricStats(avg=0.0, p50=0, p95=0, p99=0, max=0)
    return MetricStats(
        avg=sum(values) / len(values),
        p50=percentile(values, 50),
        p95=percentile(values, 95),
        p99=percentile(values, 99),
        max=max(values),
    )


def read_trace(
    path: Path, *, ignore_frames_through: int, present_width: int = 960
) -> TraceData:
    with path.open(newline="") as f:
        reader = csv.DictReader(f, delimiter="\t")
        columns = set(reader.fieldnames or [])
        if "arcade_list_present_us" not in columns and "overlay_present_us" in columns:
            columns.add("arcade_list_present_us")
        missing = sorted(REQUIRED_COLUMNS - columns)
        if missing:
            raise ValueError(f"{path}: missing required columns: {','.join(missing)}")
        rows = []
        for row in reader:
            if "arcade_list_present_us" not in row and "overlay_present_us" in row:
                row["arcade_list_present_us"] = row["overlay_present_us"]
            row.setdefault("hidden_compose_us", "0")
            row.setdefault("hidden_preview_compose_us", "0")
            row.setdefault("hidden_arcade_compose_us", "0")
            row.setdefault("direct_preview_present_us", "0")
            row.setdefault("direct_preview_rows", "0")
            row.setdefault("main_present_hidden_invalid_bytes", "0")
            row.setdefault("main_present_hidden_rect_count", "0")
            row.setdefault("main_present_hidden_catchup_bytes", "0")
            row.setdefault("main_present_hidden_full_copy", "0")
            row.setdefault("main_present_set_vga_fb_us", "0")
            if "present_bytes" not in row or not row.get("present_bytes"):
                row["present_bytes"] = str(int_field(row, "rows") * present_width * 2)
            if "wasted_present_bytes" not in row or not row.get("wasted_present_bytes"):
                dirty_rows = max(
                    0, int_field(row, "dirty_y1") - int_field(row, "dirty_y0")
                )
                dirty_bytes = dirty_rows * present_width * 2
                row["wasted_present_bytes"] = str(
                    max(0, int_field(row, "present_bytes") - dirty_bytes)
                )
            if (
                not rows
                and row.get("vsync_source", "") in ("", "none")
                and int_field(row, "vsync_miss_streak") == 0
            ):
                continue
            if int_field(row, "frame") > ignore_frames_through:
                rows.append(row)
    if not rows:
        raise ValueError(f"{path}: no rows after frame {ignore_frames_through}")
    return TraceData(path=path, rows=rows)


def grouped(rows: Iterable[dict[str, str]]) -> dict[str, list[dict[str, str]]]:
    out: dict[str, list[dict[str, str]]] = {}
    for row in rows:
        out.setdefault(row.get("arcade_update", ""), []).append(row)
    return out


def vsync_counts(rows: list[dict[str, str]]) -> dict[str, int]:
    counts = {
        "frames": len(rows),
        "vsync": 0,
        "fallback": 0,
        "timeout": 0,
        "error": 0,
        "other_source": 0,
        "max_miss_streak": 0,
    }
    for row in rows:
        source = row.get("vsync_source", "")
        if source in ("vsync", "fallback", "timeout", "error"):
            counts[source] += 1
        else:
            counts["other_source"] += 1
        counts["max_miss_streak"] = max(
            counts["max_miss_streak"], int_field(row, "vsync_miss_streak")
        )
    return counts


def clean_vsync(counts: dict[str, int]) -> bool:
    return (
        counts["fallback"] == 0
        and counts["timeout"] == 0
        and counts["error"] == 0
        and counts["other_source"] == 0
        and counts["max_miss_streak"] == 0
    )


def print_vsync(case: str, trace: TraceData, valid: bool) -> None:
    counts = vsync_counts(trace.rows)
    verdict = "pass" if valid and clean_vsync(counts) else "fail"
    print(
        "present_path_vsync_tsv"
        f"\tcase={case}"
        f"\ttrace={trace.path}"
        f"\tframes={counts['frames']}"
        f"\tvsync={counts['vsync']}"
        f"\tfallback={counts['fallback']}"
        f"\ttimeout={counts['timeout']}"
        f"\terror={counts['error']}"
        f"\tother_source={counts['other_source']}"
        f"\tmax_miss_streak={counts['max_miss_streak']}"
        f"\tverdict={verdict}"
    )


def print_summary_rows(
    trace: TraceData,
    *,
    case: str,
    min_frames: int,
    include_all: bool,
) -> int:
    print_vsync(case, trace, True)
    groups = grouped(trace.rows)
    selected = [
        (name, rows)
        for name, rows in sorted(groups.items())
        if include_all or len(rows) >= min_frames or name.startswith("scroll:")
    ]
    if not selected:
        selected = sorted(groups.items(), key=lambda item: len(item[1]), reverse=True)[
            :1
        ]
    for group, rows in selected:
        for metric in METRICS:
            stats = metric_stats(rows, metric)
            print(
                "present_path_tsv"
                f"\tmode=summarize"
                f"\tcase={case}"
                f"\ttrace={trace.path}"
                f"\tgroup={group}"
                f"\tmetric={metric}"
                f"\tframes={len(rows)}"
                f"\tavg={fmt_float(stats.avg)}"
                f"\tp50={stats.p50}"
                f"\tp95={stats.p95}"
                f"\tp99={stats.p99}"
                f"\tmax={stats.max}"
                "\tverdict=info"
            )
    return 0 if clean_vsync(vsync_counts(trace.rows)) else FAILURE_EXIT


def comparable_groups(
    before: TraceData,
    after: TraceData,
    *,
    min_frames: int,
) -> list[tuple[str, list[dict[str, str]], list[dict[str, str]]]]:
    before_groups = grouped(before.rows)
    after_groups = grouped(after.rows)
    out = []
    for group in sorted(set(before_groups) & set(after_groups)):
        b_rows = before_groups[group]
        a_rows = after_groups[group]
        if len(b_rows) >= min_frames and len(a_rows) >= min_frames:
            out.append((group, b_rows, a_rows))
    return out


def verdict_for_metric(
    metric: str,
    before: MetricStats,
    after: MetricStats,
    *,
    max_present_regression_pct: float,
    max_rows_regression: int,
) -> tuple[str, str]:
    if metric == "rows":
        p95_delta = after.p95 - before.p95
        p99_delta = after.p99 - before.p99
        if p95_delta > max_rows_regression or p99_delta > max_rows_regression:
            return "fail", (f"rows_p95_or_p99_delta_gt_{max_rows_regression}")
        return "pass", "ok"
    if metric in {"fb_present_us", "cached_present_us"}:
        p95_delta_pct = percent_delta(before.p95, after.p95)
        p99_delta_pct = percent_delta(before.p99, after.p99)
        if (
            p95_delta_pct > max_present_regression_pct
            or p99_delta_pct > max_present_regression_pct
        ):
            return "fail", (
                "present_p95_or_p99_delta_pct_gt_"
                f"{fmt_float(max_present_regression_pct)}"
            )
        return "pass", "ok"
    return "pass", "info"


def compare_traces(
    before: TraceData,
    after: TraceData,
    *,
    case: str,
    min_frames: int,
    max_present_regression_pct: float,
    max_rows_regression: int,
) -> int:
    failures = 0
    print_vsync(f"{case}:before", before, True)
    print_vsync(f"{case}:after", after, True)
    if not clean_vsync(vsync_counts(before.rows)):
        failures += 1
    if not clean_vsync(vsync_counts(after.rows)):
        failures += 1

    groups = comparable_groups(before, after, min_frames=min_frames)
    scroll_groups = [group for group, _, _ in groups if group.startswith("scroll:")]
    if not scroll_groups:
        print(
            "present_path_validity_tsv"
            f"\tcase={case}"
            "\tvalid=0"
            "\tinvalid_reason=no_comparable_scroll_group"
            f"\tmin_frames={min_frames}"
        )
        return FAILURE_EXIT

    for group, before_rows, after_rows in groups:
        for metric in METRICS:
            b = metric_stats(before_rows, metric)
            a = metric_stats(after_rows, metric)
            verdict, reason = verdict_for_metric(
                metric,
                b,
                a,
                max_present_regression_pct=max_present_regression_pct,
                max_rows_regression=max_rows_regression,
            )
            if verdict == "fail":
                failures += 1
            print(
                "present_path_tsv"
                f"\tmode=compare"
                f"\tcase={case}"
                f"\tgroup={group}"
                f"\tmetric={metric}"
                f"\tbefore_frames={len(before_rows)}"
                f"\tafter_frames={len(after_rows)}"
                f"\tbefore_avg={fmt_float(b.avg)}"
                f"\tafter_avg={fmt_float(a.avg)}"
                f"\tdelta_avg={fmt_float(a.avg - b.avg)}"
                f"\tdelta_avg_pct={fmt_float(percent_delta(b.avg, a.avg))}"
                f"\tbefore_p50={b.p50}"
                f"\tafter_p50={a.p50}"
                f"\tdelta_p50={a.p50 - b.p50}"
                f"\tdelta_p50_pct={fmt_float(percent_delta(b.p50, a.p50))}"
                f"\tbefore_p95={b.p95}"
                f"\tafter_p95={a.p95}"
                f"\tdelta_p95={a.p95 - b.p95}"
                f"\tdelta_p95_pct={fmt_float(percent_delta(b.p95, a.p95))}"
                f"\tbefore_p99={b.p99}"
                f"\tafter_p99={a.p99}"
                f"\tdelta_p99={a.p99 - b.p99}"
                f"\tdelta_p99_pct={fmt_float(percent_delta(b.p99, a.p99))}"
                f"\tbefore_max={b.max}"
                f"\tafter_max={a.max}"
                f"\tdelta_max={a.max - b.max}"
                f"\tverdict={verdict}"
                f"\treason={reason}"
            )

    valid = 1 if failures == 0 else 0
    reason = "ok" if valid else "present_path_regression"
    print(
        "present_path_validity_tsv"
        f"\tcase={case}"
        f"\tvalid={valid}"
        f"\tinvalid_reason={reason}"
        f"\tcomparable_groups={len(groups)}"
        f"\tscroll_groups={len(scroll_groups)}"
        f"\tmin_frames={min_frames}"
    )
    return 0 if valid else FAILURE_EXIT


def write_fixture(path: Path, rows: int, **values: int | str) -> None:
    defaults: dict[str, int | str] = {
        "group": "scroll:-12",
        "rows": 704,
        "direct_preview_rows": 0,
        "present_bytes": 704 * 960 * 2,
        "wasted_present_bytes": 0,
        "fb_present_us": 900,
        "cached_present_us": 400,
        "hidden_compose_us": 0,
        "hidden_preview_compose_us": 0,
        "hidden_arcade_compose_us": 0,
        "direct_preview_present_us": 0,
        "arcade_list_present_us": 500,
        "vsync_source": "vsync",
        "vsync_miss_streak": 0,
    }
    defaults.update(values)
    with path.open("w", newline="") as f:
        writer = csv.writer(f, delimiter="\t", lineterminator="\n")
        writer.writerow(
            [
                "frame",
                "arcade_update",
                "rows",
                "direct_preview_rows",
                "present_bytes",
                "wasted_present_bytes",
                "fb_present_us",
                "cached_present_us",
                "hidden_compose_us",
                "hidden_preview_compose_us",
                "hidden_arcade_compose_us",
                "direct_preview_present_us",
                "arcade_list_present_us",
                "vsync_source",
                "vsync_miss_streak",
            ]
        )
        for frame in range(rows):
            writer.writerow(
                [
                    frame,
                    defaults["group"],
                    defaults["rows"],
                    defaults["direct_preview_rows"],
                    defaults["present_bytes"],
                    defaults["wasted_present_bytes"],
                    defaults["fb_present_us"],
                    defaults["cached_present_us"],
                    defaults["hidden_compose_us"],
                    defaults["hidden_preview_compose_us"],
                    defaults["hidden_arcade_compose_us"],
                    defaults["direct_preview_present_us"],
                    defaults["arcade_list_present_us"],
                    defaults["vsync_source"],
                    defaults["vsync_miss_streak"],
                ]
            )


def run_self_test() -> int:
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        before = root / "before.tsv"
        after_good = root / "after-good.tsv"
        after_bad_copy = root / "after-bad-copy.tsv"
        after_no_scroll = root / "after-no-scroll.tsv"
        write_fixture(before, 180, cached_present_us=400, fb_present_us=900)
        write_fixture(after_good, 180, cached_present_us=410, fb_present_us=920)
        write_fixture(after_bad_copy, 180, cached_present_us=500, fb_present_us=1060)
        write_fixture(after_no_scroll, 180, group="full")

        before_trace = read_trace(before, ignore_frames_through=30)
        good_trace = read_trace(after_good, ignore_frames_through=30)
        bad_copy_trace = read_trace(after_bad_copy, ignore_frames_through=30)
        no_scroll_trace = read_trace(after_no_scroll, ignore_frames_through=30)

        if (
            compare_traces(
                before_trace,
                good_trace,
                case="self-good",
                min_frames=120,
                max_present_regression_pct=5.0,
                max_rows_regression=1,
            )
            != 0
        ):
            print("self-test: expected good comparison to pass", file=sys.stderr)
            return 1
        if (
            compare_traces(
                before_trace,
                bad_copy_trace,
                case="self-bad-copy",
                min_frames=120,
                max_present_regression_pct=5.0,
                max_rows_regression=1,
            )
            == 0
        ):
            print("self-test: expected copy regression to fail", file=sys.stderr)
            return 1
        if (
            compare_traces(
                before_trace,
                no_scroll_trace,
                case="self-no-scroll",
                min_frames=120,
                max_present_regression_pct=5.0,
                max_rows_regression=1,
            )
            == 0
        ):
            print("self-test: expected no-scroll comparison to fail", file=sys.stderr)
            return 1
    print("launcher-present-trace self-test ok")
    return 0


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--self-test", action="store_true")
    subparsers = parser.add_subparsers(dest="command")

    summarize = subparsers.add_parser("summarize")
    summarize.add_argument("trace", type=Path)
    summarize.add_argument("--case", default="arcade")
    summarize.add_argument("--min-frames", type=int, default=120)
    summarize.add_argument("--include-all-groups", action="store_true")
    summarize.add_argument("--ignore-frames-through", type=int, default=30)
    summarize.add_argument("--present-width", type=int, default=960)

    compare = subparsers.add_parser("compare")
    compare.add_argument("before", type=Path)
    compare.add_argument("after", type=Path)
    compare.add_argument("--case", default="arcade")
    compare.add_argument("--min-frames", type=int, default=120)
    compare.add_argument("--max-present-regression-pct", type=float, default=5.0)
    compare.add_argument("--max-rows-regression", type=int, default=1)
    compare.add_argument("--ignore-frames-through", type=int, default=30)
    compare.add_argument("--present-width", type=int, default=960)

    args = parser.parse_args()
    if args.self_test:
        return run_self_test()
    if args.command is None:
        parser.error("expected summarize or compare")

    try:
        if args.command == "summarize":
            trace = read_trace(
                args.trace,
                ignore_frames_through=args.ignore_frames_through,
                present_width=args.present_width,
            )
            return print_summary_rows(
                trace,
                case=args.case,
                min_frames=args.min_frames,
                include_all=args.include_all_groups,
            )
        before = read_trace(
            args.before,
            ignore_frames_through=args.ignore_frames_through,
            present_width=args.present_width,
        )
        after = read_trace(
            args.after,
            ignore_frames_through=args.ignore_frames_through,
            present_width=args.present_width,
        )
        return compare_traces(
            before,
            after,
            case=args.case,
            min_frames=args.min_frames,
            max_present_regression_pct=args.max_present_regression_pct,
            max_rows_regression=args.max_rows_regression,
        )
    except ValueError as exc:
        print(
            "present_path_validity_tsv"
            f"\tcase={getattr(args, 'case', 'unknown')}"
            "\tvalid=0"
            "\tinvalid_reason=invalid_trace"
            f"\tdetail={exc}",
        )
        print(str(exc), file=sys.stderr)
        return FAILURE_EXIT


if __name__ == "__main__":
    raise SystemExit(main())
