#!/usr/bin/env python3
# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

"""Compare matched baseline/candidate latch gate summaries."""

from __future__ import annotations

import argparse
import json
import statistics
import sys
from pathlib import Path

ZERO_FIELDS = (
    "latch_deadline_misses",
    "visual_latch_misses",
    "buffer_alternation_failures",
    "flip_counter_gaps",
    "fpga_drop_count_max",
)


def summary(path: Path) -> dict[str, str]:
    for line in path.read_text().splitlines():
        if line.startswith("max_scroll_gate_tsv"):
            fields: dict[str, str] = {}
            for token in line.replace("\t", " ").split()[1:]:
                if "=" in token:
                    key, value = token.split("=", 1)
                    fields[key] = value
            return fields
    raise ValueError(f"no max_scroll_gate_tsv row: {path}")


def check_family(
    name: str, baseline_paths: list[Path], candidate_paths: list[Path]
) -> tuple[list[str], dict[str, float]]:
    reasons: list[str] = []
    if len(baseline_paths) != 2 or len(candidate_paths) != 2:
        return [f"{name}_requires_two_baseline_and_candidate_samples"], {}
    baseline = [summary(path) for path in baseline_paths]
    candidate = [summary(path) for path in candidate_paths]
    for index, row in enumerate(candidate, 1):
        if row.get("valid") != "1":
            reasons.append(f"{name}_candidate_{index}_gate_invalid")
        for field in ZERO_FIELDS:
            if row.get(field) != "0":
                reasons.append(f"{name}_candidate_{index}_{field}")
        if row.get("fpga_counters_advanced") != "1":
            reasons.append(f"{name}_candidate_{index}_counter_stall")
    baseline_median = statistics.median(int(row["work_p99"]) for row in baseline)
    candidate_median = statistics.median(int(row["work_p99"]) for row in candidate)
    limit = baseline_median * 1.03
    if candidate_median > limit:
        reasons.append(f"{name}_p99_regression")
    return reasons, {
        f"{name}_baseline_median_p99_work_us": baseline_median,
        f"{name}_candidate_median_p99_work_us": candidate_median,
        f"{name}_candidate_limit_p99_work_us": limit,
    }


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    for family in ("home", "arcade"):
        parser.add_argument(
            f"--baseline-{family}", action="append", type=Path, required=True
        )
        parser.add_argument(
            f"--candidate-{family}", action="append", type=Path, required=True
        )
    parser.add_argument("--json", action="store_true")
    args = parser.parse_args()
    reasons: list[str] = []
    details: dict[str, float] = {}
    for family in ("home", "arcade"):
        family_reasons, family_details = check_family(
            family,
            getattr(args, f"baseline_{family}"),
            getattr(args, f"candidate_{family}"),
        )
        reasons.extend(family_reasons)
        details.update(family_details)
    payload = {
        "valid": int(not reasons),
        "invalid_reason": ",".join(reasons) or "ok",
        **details,
    }
    if args.json:
        print(json.dumps(payload, sort_keys=True, separators=(",", ":")))
    else:
        print(
            "fpga_device_qualification_tsv\t"
            + "\t".join(f"{key}={value}" for key, value in payload.items())
        )
    return 0 if not reasons else 1


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except (OSError, ValueError, KeyError) as error:
        print(f"qualification summary failed: {error}", file=sys.stderr)
        raise SystemExit(1)
