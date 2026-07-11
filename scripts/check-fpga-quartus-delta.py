#!/usr/bin/env python3
"""Compare stock and patched Quartus reports for FPGA release signoff."""

from __future__ import annotations

import argparse
from collections import Counter
import json
import math
from pathlib import Path
import re
import sys
from typing import Iterable


NUMBER = r"[-+]?(?:\d+(?:\.\d*)?|\.\d+)(?:[eE][-+]?\d+)?"
WARNING_RE = re.compile(r"^\s*Warning(?: \((\d+)\))?:\s*(.*?)\s*$", re.IGNORECASE)
SLACK_RE = re.compile(
    rf"Worst-case\s+(setup|hold)\s+slack\s+is\s+({NUMBER})", re.IGNORECASE
)
TNS_RE = re.compile(rf"Total\s+negative\s+slack(?:\s+is|\s*[:=])\s*({NUMBER})", re.IGNORECASE)
TABLE_ROW_RE = re.compile(rf"^\s*(?:Info \(\d+\):\s*)?({NUMBER})\s+({NUMBER})\s+\S")
CHAIN_COUNT_RE = re.compile(r"(?:Found|Found:)\s+(\d+)\s+synchronizer chains", re.IGNORECASE)
MTBF_VALUE_RE = re.compile(rf"(?:MTBF|Mean Time Between Failures).*?({NUMBER})\s*(years?|seconds?|s)\b", re.IGNORECASE)
UNCONSTRAINED_RE = re.compile(r"\bunconstrained\b|not fully constrained", re.IGNORECASE)
UNCONSTRAINED_OUTPUT_RE = re.compile(r"Unconstrained Output Port Paths\s*;\s*(\d+)\s*;", re.IGNORECASE)
UNCALCULATED_FRACTION_RE = re.compile(r"Fraction of Chains for which MTBFs Could Not be Calculated:\s*([0-9.]+)", re.IGNORECASE)
SYNC_ASSIGN_RE = re.compile(r"SYNCHRONIZER_IDENTIFICATION\s*;\s*FORCED_IF_ASYNCHRONOUS.*;\s*(vbl_meta|vbl_sys)\s*;", re.IGNORECASE)
RESOURCE_RE = re.compile(
    r"^\s*(Total (?:logic elements|registers|block memory bits|DSP Blocks))\s*[:;]\s*([\d,]+)",
    re.IGNORECASE,
)


def normalize_space(value: str) -> str:
    value = re.sub(r"\s+File:\s+\S+\s+Line:\s+\d+\s*$", "", value, flags=re.IGNORECASE)
    value = re.sub(r'Node "emu\|random\|lc0\|data[a-f]"', 'Node "emu|random|lc0|data*"', value, flags=re.IGNORECASE)
    return " ".join(value.split())


def warning_identity(match: re.Match[str]) -> str:
    code = match.group(1) or "none"
    return f"{code}:{normalize_space(match.group(2))}"


def read_inputs(paths: Iterable[Path]) -> str:
    chunks: list[str] = []
    for path in paths:
        try:
            chunks.append(path.read_text(encoding="utf-8", errors="replace"))
        except OSError as error:
            raise ValueError(f"cannot read {path}: {error}") from error
    return "\n".join(chunks)


def finite_number(value: str) -> float | None:
    try:
        parsed = float(value)
    except ValueError:
        return None
    return parsed if math.isfinite(parsed) else None


def parse_report(text: str, synchronizer_re: re.Pattern[str]) -> dict[str, object]:
    lines = text.splitlines()
    warnings: Counter[str] = Counter()
    slacks: dict[str, list[float]] = {"setup": [], "hold": []}
    tns: list[float] = []
    unconstrained: Counter[str] = Counter()
    chain_counts: list[int] = []
    custom_sync_lines: list[int] = []
    custom_sync_mtbf = False
    resources: dict[str, int] = {}
    sync_assignments: set[str] = set()
    uncalculated_fractions: list[float] = []
    unconstrained_output_paths: list[int] = []
    timing_section: str | None = None
    in_tns_table = False

    for index, line in enumerate(lines):
        warning = WARNING_RE.match(line)
        if warning:
            warnings[warning_identity(warning)] += 1

        slack = SLACK_RE.search(line)
        if slack:
            value = finite_number(slack.group(2))
            if value is not None:
                timing_section = slack.group(1).lower()
                slacks[timing_section].append(value)
                in_tns_table = False

        if "Slack" in line and "TNS" in line and timing_section in ("setup", "hold"):
            in_tns_table = True
            continue
        if in_tns_table:
            row = TABLE_ROW_RE.match(line)
            if row:
                value = finite_number(row.group(2))
                if value is not None:
                    tns.append(value)
                continue
            if line.strip() and "====" not in line:
                in_tns_table = False

        total_tns = TNS_RE.search(line)
        if total_tns:
            value = finite_number(total_tns.group(1))
            if value is not None:
                tns.append(value)

        unconstrained_output = UNCONSTRAINED_OUTPUT_RE.search(line)
        if unconstrained_output:
            unconstrained_output_paths.append(int(unconstrained_output.group(1)))
        elif UNCONSTRAINED_RE.search(line):
            unconstrained[normalize_space(line)] += 1

        chains = CHAIN_COUNT_RE.search(line)
        if chains:
            chain_counts.append(int(chains.group(1)))

        if synchronizer_re.search(line):
            custom_sync_lines.append(index)

        sync_assignment = SYNC_ASSIGN_RE.search(line)
        if sync_assignment:
            sync_assignments.add(sync_assignment.group(1).lower())

        fraction = UNCALCULATED_FRACTION_RE.search(line)
        if fraction:
            value = finite_number(fraction.group(1))
            if value is not None:
                uncalculated_fractions.append(value)

        resource = RESOURCE_RE.search(line)
        if resource:
            resources[normalize_space(resource.group(1)).lower()] = int(resource.group(2).replace(",", ""))

    for index in custom_sync_lines:
        window = " ".join(lines[index : min(index + 6, len(lines))])
        match = MTBF_VALUE_RE.search(window)
        if match:
            value = finite_number(match.group(1))
            if value is not None and value > 0:
                custom_sync_mtbf = True
                break

    return {
        "warnings": warnings,
        "slacks": slacks,
        "tns": tns,
        "unconstrained": unconstrained,
        "chain_counts": chain_counts,
        "custom_sync_seen": bool(custom_sync_lines),
        "custom_sync_mtbf": custom_sync_mtbf,
        "resources": resources,
        "sync_assignments": sync_assignments,
        "uncalculated_fractions": uncalculated_fractions,
        "unconstrained_output_paths": unconstrained_output_paths,
    }


def counter_delta(left: Counter[str], right: Counter[str]) -> list[str]:
    result: list[str] = []
    for identity, count in sorted((right - left).items()):
        result.append(f"{count}x {identity}")
    return result


def compare(stock: dict[str, object], patched: dict[str, object]) -> tuple[list[str], dict[str, object]]:
    reasons: list[str] = []
    stock_warnings = stock["warnings"]
    patched_warnings = patched["warnings"]
    assert isinstance(stock_warnings, Counter) and isinstance(patched_warnings, Counter)
    added_warnings = counter_delta(stock_warnings, patched_warnings)
    removed_warnings = counter_delta(patched_warnings, stock_warnings)
    if added_warnings:
        reasons.append("warning_added")
    if removed_warnings:
        reasons.append("warning_baseline_mismatch")

    stock_unconstrained = stock["unconstrained"]
    patched_unconstrained = patched["unconstrained"]
    assert isinstance(stock_unconstrained, Counter) and isinstance(patched_unconstrained, Counter)
    added_unconstrained = counter_delta(stock_unconstrained, patched_unconstrained)
    if added_unconstrained:
        reasons.append("unconstrained_added")
    stock_output_paths = stock["unconstrained_output_paths"]
    patched_output_paths = patched["unconstrained_output_paths"]
    assert isinstance(stock_output_paths, list) and isinstance(patched_output_paths, list)
    if not stock_output_paths or not patched_output_paths:
        reasons.append("unconstrained_output_summary_missing")
    elif max(patched_output_paths) > max(stock_output_paths):
        reasons.append("unconstrained_output_paths_added")

    slacks = patched["slacks"]
    assert isinstance(slacks, dict)
    for kind in ("setup", "hold"):
        values = slacks[kind]
        if not values:
            reasons.append(f"{kind}_slack_missing")
        elif min(values) < 0:
            reasons.append(f"{kind}_slack_negative")

    tns = patched["tns"]
    assert isinstance(tns, list)
    if not tns:
        reasons.append("tns_missing")
    elif any(abs(value) > 1e-12 for value in tns):
        reasons.append("tns_nonzero")

    chain_counts = patched["chain_counts"]
    assert isinstance(chain_counts, list)
    if not chain_counts or max(chain_counts) <= 0:
        reasons.append("synchronizer_report_missing")
    stock_chain_counts = stock["chain_counts"]
    assert isinstance(stock_chain_counts, list)
    sync_assignments = patched["sync_assignments"]
    assert isinstance(sync_assignments, set)
    custom_assignment_seen = sync_assignments == {"vbl_meta", "vbl_sys"}
    stock_fractions = stock["uncalculated_fractions"]
    patched_fractions = patched["uncalculated_fractions"]
    assert isinstance(stock_fractions, list) and isinstance(patched_fractions, list)
    custom_delta_calculable = (
        bool(stock_chain_counts)
        and bool(chain_counts)
        and max(chain_counts) == max(stock_chain_counts) + 1
        and bool(stock_fractions)
        and bool(patched_fractions)
        and min(patched_fractions) < min(stock_fractions)
    )
    if not custom_assignment_seen:
        reasons.append("custom_synchronizer_missing")
    elif not custom_delta_calculable:
        reasons.append("custom_synchronizer_mtbf_missing")

    details = {
        "stock_warning_count": sum(stock_warnings.values()),
        "patched_warning_count": sum(patched_warnings.values()),
        "added_warnings": added_warnings,
        "removed_warnings": removed_warnings,
        "added_unconstrained": added_unconstrained,
        "patched_setup_slack_min": min(slacks["setup"]) if slacks["setup"] else None,
        "patched_hold_slack_min": min(slacks["hold"]) if slacks["hold"] else None,
        "patched_tns_max_abs": max((abs(value) for value in tns), default=None),
        "patched_synchronizer_chains": max(chain_counts, default=None),
        "custom_sync_seen": custom_assignment_seen,
        "custom_sync_mtbf": custom_delta_calculable,
        "stock_unconstrained_output_paths": max(stock_output_paths, default=None),
        "patched_unconstrained_output_paths": max(patched_output_paths, default=None),
        "stock_resources": stock["resources"],
        "patched_resources": patched["resources"],
    }
    return sorted(set(reasons)), details


def tsv_value(value: object) -> str:
    if value is None:
        return "missing"
    if isinstance(value, bool):
        return "1" if value else "0"
    return str(value).replace("\t", " ").replace("\n", " ")


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--stock", action="append", type=Path, required=True, help="stock log/report; repeatable")
    parser.add_argument("--patched", action="append", type=Path, required=True, help="patched log/report; repeatable")
    parser.add_argument(
        "--synchronizer-regex",
        default=r"mister_magik_vblank_latch.*(?:vbl_meta|vbl_sys)|(?:vbl_meta|vbl_sys).*mister_magik_vblank_latch",
        help="case-insensitive regex identifying the custom synchronizer report row",
    )
    parser.add_argument("--json", action="store_true", help="emit JSON instead of TSV")
    args = parser.parse_args(argv)

    try:
        sync_re = re.compile(args.synchronizer_regex, re.IGNORECASE)
        stock = parse_report(read_inputs(args.stock), sync_re)
        patched = parse_report(read_inputs(args.patched), sync_re)
    except (ValueError, re.error) as error:
        parser.error(str(error))

    reasons, details = compare(stock, patched)
    valid = not reasons
    result = {"valid": int(valid), "invalid_reason": ",".join(reasons) if reasons else "ok", **details}
    if args.json:
        print(json.dumps(result, sort_keys=True, separators=(",", ":")))
    else:
        fields = ["quartus_delta_signoff_tsv"] + [f"{key}={tsv_value(value)}" for key, value in result.items() if not isinstance(value, list)]
        print("\t".join(fields))
        for key in ("added_warnings", "removed_warnings", "added_unconstrained"):
            for value in details[key]:
                print(f"quartus_delta_detail_tsv\tkind={key}\tvalue={tsv_value(value)}")
    return 0 if valid else 1


if __name__ == "__main__":
    sys.exit(main())
