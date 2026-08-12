#!/usr/bin/env python3
# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

"""Compare stock, pre-observer, and final Quartus reports for FPGA signoff."""

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
SYNC_ASSIGN_RE = re.compile(
    r"SYNCHRONIZER_IDENTIFICATION\s*;\s*FORCED(?:_IF_ASYNCHRONOUS)?\s*;",
    re.IGNORECASE,
)
SOURCE_ASSIGNMENTS_RE = re.compile(
    r"^\s*;\s*Source assignments for\s+(.+?)\s*;\s*$",
    re.IGNORECASE,
)
RESOURCE_RE = re.compile(
    r"^\s*;?\s*((?:Logic utilization \(in ALMs\)|Total (?:logic elements|registers|block memory bits|DSP Blocks)))\s*[:;]\s*([\d,]+)",
    re.IGNORECASE,
)
BOOTSTRAP_BLACK_LOOP_WARNING = "332125:Found combinational loop of 6 nodes"
BOOTSTRAP_BLACK_COMBOUT_WARNING = '332126:Node "emu|random|lc0|combout"'
BOOTSTRAP_BLACK_DATA_WARNING = '332126:Node "emu|random|lc0|data*"'
EXPECTED_BOOTSTRAP_BLACK_REMOVED_WARNING_IDENTITIES = frozenset(
    (
        BOOTSTRAP_BLACK_LOOP_WARNING,
        BOOTSTRAP_BLACK_COMBOUT_WARNING,
        BOOTSTRAP_BLACK_DATA_WARNING,
    )
)
MINIMUM_SLACK_NS = 0.20
MAXIMUM_SLACK_DEGRADATION_NS = 0.15
MAXIMUM_LOGIC_ELEMENT_DELTA = 800
MAXIMUM_REGISTER_DELTA = 96
MINIMUM_OBSERVER_REPORTED_CHAINS = 1
EXPECTED_OBSERVER_CALCULABLE_CHAINS = 1
EXPECTED_SYNC_ASSIGNMENT_SUFFIXES = (
    "mister_magik_hdmi_lock_evidence:magik_hdmi_lock_evidence|control_pll_lock_meta",
    "mister_magik_hdmi_lock_evidence:magik_hdmi_lock_evidence|control_pll_lock_sys",
)
EXPECTED_CDC_ANALYSIS_LABELS: frozenset[str] = frozenset()
DIAGNOSTIC_REPORT_NAMES = frozenset(
    {
        "menu.magik-diagnostic-cdc-skew.rpt",
        "menu.magik-diagnostic-cdc-net-delay.rpt",
        "menu.magik-diagnostic-metastability.rpt",
    }
)
EXPECTED_CDC_REPORT_ANALYSES = {
    "menu.magik-diagnostic-cdc-skew.rpt": "set_max_skew",
    "menu.magik-diagnostic-cdc-net-delay.rpt": "set_net_delay",
}


def normalize_space(value: str) -> str:
    value = re.sub(r"\s+File:\s+\S+\s+Line:\s+\d+\s*$", "", value, flags=re.IGNORECASE)
    value = re.sub(r'Node "emu\|random\|lc0\|data[a-f]"', 'Node "emu|random|lc0|data*"', value, flags=re.IGNORECASE)
    return " ".join(value.split())


def warning_identity(match: re.Match[str]) -> str:
    code = match.group(1) or "none"
    return f"{code}:{normalize_space(match.group(2))}"


def parse_sync_assignments(lines: Iterable[str]) -> set[str]:
    assignments: set[str] = set()
    hierarchy: str | None = None
    for line in lines:
        source_assignments = SOURCE_ASSIGNMENTS_RE.match(line)
        if source_assignments:
            hierarchy = normalize_space(source_assignments.group(1))
            continue
        if not SYNC_ASSIGN_RE.search(line):
            continue
        fields = [normalize_space(field) for field in line.split(";") if field.strip()]
        if not fields:
            continue
        target = fields[-1]
        if hierarchy and "|" not in target:
            target = f"{hierarchy}|{target}"
        assignments.add(target.lower())
    return assignments


def read_inputs(paths: Iterable[Path]) -> tuple[str, str | None, dict[str, str]]:
    chunks: list[str] = []
    fitter_summaries: list[str] = []
    diagnostic_reports: dict[str, str] = {}
    for path in paths:
        try:
            text = path.read_text(encoding="utf-8", errors="replace")
        except OSError as error:
            raise ValueError(f"cannot read {path}: {error}") from error
        if path.name in DIAGNOSTIC_REPORT_NAMES:
            if path.name in diagnostic_reports:
                raise ValueError(f"duplicate diagnostic timing report: {path.name}")
            diagnostic_reports[path.name] = text
            continue
        chunks.append(text)
        if path.name.endswith(".fit.summary"):
            fitter_summaries.append(text)
    return "\n".join(chunks), "\n".join(fitter_summaries) or None, diagnostic_reports


def finite_number(value: str) -> float | None:
    try:
        parsed = float(value)
    except ValueError:
        return None
    return parsed if math.isfinite(parsed) else None


def parse_report(
    text: str,
    synchronizer_re: re.Pattern[str],
    fitter_summary: str | None = None,
) -> dict[str, object]:
    lines = text.splitlines()
    warnings: Counter[str] = Counter()
    slacks: dict[str, list[float]] = {"setup": [], "hold": []}
    tns: list[float] = []
    unconstrained: Counter[str] = Counter()
    chain_counts: list[int] = []
    custom_sync_lines: list[int] = []
    custom_sync_mtbf = False
    resources: dict[str, int] = {}
    sync_assignments = parse_sync_assignments(lines)
    diagnostic_analysis_labels: Counter[str] = Counter()
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

        analysis_label = re.search(
            r"MagiK diagnostics CDC analysis applied:\s*([a-z_]+)", line
        )
        if analysis_label:
            diagnostic_analysis_labels[analysis_label.group(1)] += 1

        fraction = UNCALCULATED_FRACTION_RE.search(line)
        if fraction:
            value = finite_number(fraction.group(1))
            if value is not None:
                uncalculated_fractions.append(value)

    # Aggregated inputs also contain Analysis & Synthesis estimates. Resource
    # budgets must use the final fitter summary when Quartus emitted one.
    for line in (fitter_summary or text).splitlines():
        resource = RESOURCE_RE.search(line)
        if resource:
            resources[normalize_space(resource.group(1)).lower()] = int(
                resource.group(2).replace(",", "")
            )

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
        "diagnostic_analysis_labels": diagnostic_analysis_labels,
        "uncalculated_fractions": uncalculated_fractions,
        "unconstrained_output_paths": unconstrained_output_paths,
    }


def counter_delta(left: Counter[str], right: Counter[str]) -> list[str]:
    result: list[str] = []
    for identity, count in sorted((right - left).items()):
        result.append(f"{count}x {identity}")
    return result


def is_expected_bootstrap_black_warning_removal(
    removed: Counter[str], patched: Counter[str]
) -> bool:
    """Recognize only the stock Menu random-loop warnings pruned by black RGB."""
    if set(removed) != EXPECTED_BOOTSTRAP_BLACK_REMOVED_WARNING_IDENTITIES:
        return False
    loop_count = removed[BOOTSTRAP_BLACK_LOOP_WARNING]
    return (
        loop_count in (5, 7)
        and removed[BOOTSTRAP_BLACK_COMBOUT_WARNING] == loop_count
        and removed[BOOTSTRAP_BLACK_DATA_WARNING] == loop_count * 5
        and not any(patched[identity] for identity in EXPECTED_BOOTSTRAP_BLACK_REMOVED_WARNING_IDENTITIES)
    )


def estimated_calculable_chains(chain_counts: list[int], uncalculated_fractions: list[float]) -> int | None:
    if not chain_counts or not uncalculated_fractions:
        return None
    total = max(chain_counts)
    uncalculated_fraction = min(uncalculated_fractions)
    return int(math.floor(total * (1.0 - uncalculated_fraction) + 0.5))


def validate_diagnostic_reports(
    reports: dict[str, str], analysis_labels: Counter[str]
) -> tuple[list[str], dict[str, object]]:
    reasons: list[str] = []
    missing_reports = sorted(DIAGNOSTIC_REPORT_NAMES - reports.keys())
    unexpected_reports = sorted(reports.keys() - DIAGNOSTIC_REPORT_NAMES)
    if missing_reports or unexpected_reports:
        reasons.append("diagnostic_cdc_report_missing")

    labels_valid = set(analysis_labels) == EXPECTED_CDC_ANALYSIS_LABELS
    if not labels_valid:
        reasons.append("diagnostic_cdc_analysis_missing")

    analysis_counts: dict[str, int | None] = {}
    minimum_slacks: dict[str, float | None] = {}
    for name, command in EXPECTED_CDC_REPORT_ANALYSES.items():
        text = reports.get(name, "")
        # Lock-only v1 deliberately has no bundled-data CDC assignment. Keep
        # the legacy report files for cache identity and prove they contain no
        # applied max-skew/net-delay rows.
        summary_rows = list(
            re.finditer(
                rf"(?m)^\s*;\s*{command}\s*;\s*({NUMBER})\s*;",
                text,
                re.IGNORECASE,
            )
        )
        analysis_counts[name] = len(summary_rows)
        if summary_rows:
            reasons.append("diagnostic_cdc_analysis_count")
        slacks = [finite_number(row.group(1)) for row in summary_rows]
        finite_slacks = [value for value in slacks if value is not None]
        if len(finite_slacks) != len(summary_rows):
            reasons.append("diagnostic_cdc_slack_missing")
        minimum_slacks[name] = min(finite_slacks, default=None)

    metastability = reports.get("menu.magik-diagnostic-metastability.rpt", "")
    metastability_lower = metastability.lower()
    missing_metastability_chains = [
        suffix
        for suffix in EXPECTED_SYNC_ASSIGNMENT_SUFFIXES
        if suffix not in metastability_lower
    ]
    if not metastability.strip() or re.search(
        r"\bno (?:valid )?(?:chains?|results?)\b", metastability, re.IGNORECASE
    ):
        reasons.append("diagnostic_metastability_report_missing")
    elif missing_metastability_chains:
        reasons.append("diagnostic_metastability_chain_missing")

    return sorted(set(reasons)), {
        "diagnostic_cdc_reports": sorted(reports),
        "diagnostic_cdc_analysis_labels": dict(sorted(analysis_labels.items())),
        "diagnostic_cdc_analysis_counts": analysis_counts,
        "diagnostic_cdc_minimum_slacks": minimum_slacks,
        "missing_diagnostic_metastability_chains": missing_metastability_chains,
    }


def compare(
    stock: dict[str, object],
    baseline: dict[str, object],
    patched: dict[str, object],
) -> tuple[list[str], dict[str, object]]:
    reasons: list[str] = []
    # Warning, constraint-identity, and CDC checks describe functional drift
    # from upstream Menu. Observer cost is the final build relative to the
    # exact patched latch build before the observer was introduced.
    stock_warnings = stock["warnings"]
    patched_warnings = patched["warnings"]
    assert isinstance(stock_warnings, Counter) and isinstance(patched_warnings, Counter)
    added_warnings = counter_delta(stock_warnings, patched_warnings)
    removed_warnings = counter_delta(patched_warnings, stock_warnings)
    removed_warning_counts = stock_warnings - patched_warnings
    expected_bootstrap_black_warning_removal = is_expected_bootstrap_black_warning_removal(
        removed_warning_counts, patched_warnings
    )
    if added_warnings:
        reasons.append("warning_added")
    if removed_warnings and not expected_bootstrap_black_warning_removal:
        reasons.append("warning_baseline_mismatch")

    stock_unconstrained = stock["unconstrained"]
    patched_unconstrained = patched["unconstrained"]
    assert isinstance(stock_unconstrained, Counter) and isinstance(patched_unconstrained, Counter)
    added_unconstrained = counter_delta(stock_unconstrained, patched_unconstrained)
    if added_unconstrained:
        reasons.append("unconstrained_added")
    baseline_output_paths = baseline["unconstrained_output_paths"]
    patched_output_paths = patched["unconstrained_output_paths"]
    assert isinstance(baseline_output_paths, list) and isinstance(patched_output_paths, list)
    if not baseline_output_paths or not patched_output_paths:
        reasons.append("unconstrained_output_summary_missing")
    elif max(patched_output_paths) > max(baseline_output_paths):
        reasons.append("unconstrained_output_paths_added")

    slacks = patched["slacks"]
    baseline_slacks = baseline["slacks"]
    assert isinstance(slacks, dict)
    assert isinstance(baseline_slacks, dict)
    for kind in ("setup", "hold"):
        values = slacks[kind]
        baseline_values = baseline_slacks[kind]
        if not baseline_values:
            reasons.append(f"baseline_{kind}_slack_missing")
        if not values:
            reasons.append(f"{kind}_slack_missing")
        else:
            patched_min = min(values)
            if patched_min < MINIMUM_SLACK_NS:
                reasons.append(f"{kind}_slack_below_minimum")
            if (
                baseline_values
                and min(baseline_values) - patched_min > MAXIMUM_SLACK_DEGRADATION_NS
            ):
                reasons.append(f"{kind}_slack_degradation")

    baseline_resources = baseline["resources"]
    patched_resources = patched["resources"]
    assert isinstance(baseline_resources, dict) and isinstance(patched_resources, dict)
    logic_resource = (
        "logic utilization (in alms)"
        if "logic utilization (in alms)" in baseline_resources
        or "logic utilization (in alms)" in patched_resources
        else "total logic elements"
    )
    resource_limits = {
        logic_resource: MAXIMUM_LOGIC_ELEMENT_DELTA,
        "total registers": MAXIMUM_REGISTER_DELTA,
        "total block memory bits": 0,
        "total dsp blocks": 0,
    }
    resource_deltas: dict[str, int | None] = {}
    for resource, limit in resource_limits.items():
        if resource not in baseline_resources or resource not in patched_resources:
            reasons.append("resource_summary_missing")
            resource_deltas[resource] = None
            continue
        delta = patched_resources[resource] - baseline_resources[resource]
        resource_deltas[resource] = delta
        if delta > limit:
            reasons.append(
                "logic_alms_delta"
                if resource == "logic utilization (in alms)"
                else resource.replace("total ", "").replace(" ", "_") + "_delta"
            )

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
    baseline_chain_counts = baseline["chain_counts"]
    assert isinstance(baseline_chain_counts, list)
    sync_assignments = patched["sync_assignments"]
    assert isinstance(sync_assignments, set)
    missing_sync_assignments = [
        suffix
        for suffix in EXPECTED_SYNC_ASSIGNMENT_SUFFIXES
        if not any(assignment.endswith(suffix) for assignment in sync_assignments)
    ]
    custom_assignment_seen = not missing_sync_assignments
    baseline_fractions = baseline["uncalculated_fractions"]
    patched_fractions = patched["uncalculated_fractions"]
    assert isinstance(baseline_fractions, list) and isinstance(patched_fractions, list)
    baseline_calculable_chains = estimated_calculable_chains(
        baseline_chain_counts, baseline_fractions
    )
    patched_calculable_chains = estimated_calculable_chains(chain_counts, patched_fractions)
    chain_delta_sufficient = (
        baseline_chain_counts
        and chain_counts
        and max(chain_counts)
        >= max(baseline_chain_counts) + MINIMUM_OBSERVER_REPORTED_CHAINS
    )
    custom_delta_calculable = (
        baseline_calculable_chains is not None
        and patched_calculable_chains is not None
        and patched_calculable_chains
        == baseline_calculable_chains + EXPECTED_OBSERVER_CALCULABLE_CHAINS
    )
    if not custom_assignment_seen:
        reasons.append("custom_synchronizer_missing")
    if not chain_delta_sufficient:
        reasons.append("custom_synchronizer_chain_count")
    if not custom_delta_calculable:
        reasons.append("custom_synchronizer_mtbf_missing")

    analysis_labels = patched["diagnostic_analysis_labels"]
    diagnostic_reports = patched["diagnostic_reports"]
    assert isinstance(analysis_labels, Counter) and isinstance(diagnostic_reports, dict)
    diagnostic_reasons, diagnostic_details = validate_diagnostic_reports(
        diagnostic_reports, analysis_labels
    )
    reasons.extend(diagnostic_reasons)

    stock_output_paths = stock["unconstrained_output_paths"]
    assert isinstance(stock_output_paths, list)
    details = {
        "stock_warning_count": sum(stock_warnings.values()),
        "patched_warning_count": sum(patched_warnings.values()),
        "added_warnings": added_warnings,
        "removed_warnings": removed_warnings,
        "expected_bootstrap_black_warning_removal": expected_bootstrap_black_warning_removal,
        "added_unconstrained": added_unconstrained,
        "patched_setup_slack_min": min(slacks["setup"]) if slacks["setup"] else None,
        "patched_hold_slack_min": min(slacks["hold"]) if slacks["hold"] else None,
        "patched_tns_max_abs": max((abs(value) for value in tns), default=None),
        "patched_synchronizer_chains": max(chain_counts, default=None),
        "baseline_synchronizer_chains": max(baseline_chain_counts, default=None),
        "baseline_calculable_synchronizer_chains": baseline_calculable_chains,
        "patched_calculable_synchronizer_chains": patched_calculable_chains,
        "missing_sync_assignments": missing_sync_assignments,
        "custom_sync_seen": custom_assignment_seen,
        "custom_sync_mtbf": custom_delta_calculable,
        "stock_unconstrained_output_paths": max(stock_output_paths, default=None),
        "baseline_unconstrained_output_paths": max(baseline_output_paths, default=None),
        "patched_unconstrained_output_paths": max(patched_output_paths, default=None),
        "stock_resources": stock["resources"],
        "baseline_resources": baseline_resources,
        "patched_resources": patched["resources"],
        "resource_deltas": resource_deltas,
        **diagnostic_details,
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
        "--baseline",
        action="append",
        type=Path,
        required=True,
        help="exact pre-observer patched log/report; repeatable",
    )
    parser.add_argument(
        "--synchronizer-regex",
        default=r"mister_magik_vblank_latch.*(?:vbl_meta|vbl_sys)|(?:vbl_meta|vbl_sys).*mister_magik_vblank_latch",
        help="case-insensitive regex identifying the custom synchronizer report row",
    )
    parser.add_argument("--json", action="store_true", help="emit JSON instead of TSV")
    args = parser.parse_args(argv)

    try:
        sync_re = re.compile(args.synchronizer_regex, re.IGNORECASE)
        stock_text, stock_fitter, stock_diagnostic_reports = read_inputs(args.stock)
        baseline_text, baseline_fitter, baseline_diagnostic_reports = read_inputs(args.baseline)
        patched_text, patched_fitter, patched_diagnostic_reports = read_inputs(args.patched)
        stock = parse_report(stock_text, sync_re, stock_fitter)
        baseline = parse_report(baseline_text, sync_re, baseline_fitter)
        patched = parse_report(patched_text, sync_re, patched_fitter)
        stock["diagnostic_reports"] = stock_diagnostic_reports
        baseline["diagnostic_reports"] = baseline_diagnostic_reports
        patched["diagnostic_reports"] = patched_diagnostic_reports
    except (ValueError, re.error) as error:
        parser.error(str(error))

    reasons, details = compare(stock, baseline, patched)
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
