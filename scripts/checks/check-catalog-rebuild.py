#!/usr/bin/env python3
# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

"""Validate and report the Catalog V3 full-versus-delta rebuild result."""

from __future__ import annotations

import sys
from pathlib import Path


def fail(label: str, reason: str, detail: str) -> int:
    print(
        f"catalog_rebuild_gate_tsv\tlabel={label}\tvalid=0"
        f"\tinvalid_reason={reason}\tdetail={detail}"
    )
    return 9


def check(label: str, report: Path, minimum_speedup: float = 10.0) -> int:
    try:
        lines = report.read_text().splitlines()
    except FileNotFoundError:
        return fail(label, "missing_report", str(report))
    row = next((line for line in lines if line.startswith("catalog_rebuild_bench_tsv\t")), None)
    if row is None:
        return fail(label, "missing_benchmark_row", str(report))
    fields = dict(field.split("=", 1) for field in row.split("\t")[1:] if "=" in field)
    required = {"full_us", "delta_us", "elapsed_speedup", "full_systems", "delta_systems", "work_ratio"}
    missing = sorted(required - set(fields))
    if missing:
        return fail(label, "missing_field", ",".join(missing))
    try:
        full_us = int(fields["full_us"])
        delta_us = int(fields["delta_us"])
        elapsed = float(fields["elapsed_speedup"])
        full_systems = int(fields["full_systems"])
        delta_systems = int(fields["delta_systems"])
        work = float(fields["work_ratio"])
    except ValueError as error:
        return fail(label, "invalid_number", str(error))
    valid = (
        full_us > 0
        and delta_us > 0
        and full_systems >= 10
        and delta_systems > 0
        and delta_systems < full_systems
        and work > 1.0
    )
    target_met = elapsed >= minimum_speedup and work >= minimum_speedup
    print(
        f"catalog_rebuild_gate_tsv\tlabel={label}\tvalid={int(valid)}"
        f"\tinvalid_reason={'ok' if valid else 'below_target'}"
        f"\tfull_us={full_us}\tdelta_us={delta_us}\telapsed_speedup={elapsed:.3f}"
        f"\tfull_systems={full_systems}\tdelta_systems={delta_systems}"
        f"\twork_ratio={work:.3f}\ttarget_speedup={minimum_speedup:.3f}"
        f"\ttarget_met={int(target_met)}"
    )
    return 0 if valid else 9


def self_test() -> int:
    import tempfile

    with tempfile.TemporaryDirectory() as directory:
        report = Path(directory) / "report.tsv"
        report.write_text(
            "catalog_rebuild_bench_tsv\tfull_us=1000\tdelta_us=100"
            "\telapsed_speedup=10.000\tfull_systems=10\tdelta_systems=1\twork_ratio=10.000\n"
        )
        if check("self-pass", report) != 0:
            return 1
        report.write_text(report.read_text().replace("elapsed_speedup=10.000", "elapsed_speedup=9.999"))
        if check("self-report", report) != 0:
            print("self-test rejected a structurally valid sub-10x rebuild", file=sys.stderr)
            return 1
    print("catalog rebuild checker self-test ok")
    return 0


def main(argv: list[str]) -> int:
    if argv[1:] == ["--self-test"]:
        return self_test()
    if len(argv) not in (3, 4):
        print("usage: check-catalog-rebuild.py LABEL REPORT [MINIMUM_SPEEDUP]", file=sys.stderr)
        return 2
    return check(argv[1], Path(argv[2]), float(argv[3]) if len(argv) == 4 else 10.0)


if __name__ == "__main__":
    sys.exit(main(sys.argv))
