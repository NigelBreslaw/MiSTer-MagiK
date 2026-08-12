#!/usr/bin/env python3
# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

"""Fixture tests for check-fpga-quartus-delta.py."""

from __future__ import annotations

import json
from pathlib import Path
import subprocess
import tempfile
import unittest


SCRIPT = Path(__file__).resolve().parents[1] / "checks/check-fpga-quartus-delta.py"

BASE = """\
Warning (10001): inherited warning File: /work/sys/top.v Line: 7
Info (332146): Worst-case setup slack is 0.232
    Info (332119):     Slack       End Point TNS Clock
    Info (332119): ========= =================== =====================
    Info (332119):     0.232               0.000 clk_sys
Info (332146): Worst-case hold slack is 0.249
    Info (332119):     Slack       End Point TNS Clock
    Info (332119): ========= =================== =====================
    Info (332119):     0.249               0.000 clk_sys
Info (332114): Report Metastability: Found 5 synchronizer chains.
Info (332114): Fraction of Chains for which MTBFs Could Not be Calculated: 0.800
; Unconstrained Output Port Paths ; 10 ; 10 ;
Info (332102): Design is not fully constrained for setup requirements
Info (332102): Design is not fully constrained for hold requirements
Total logic elements: 10,000
Total registers: 20,000
Total block memory bits: 1,000,000
Total DSP Blocks: 2
"""

CONTROL_SYNC_NAMES = (
    "avalon_fault_meta", "avalon_fault_sys", "output_fault_meta", "output_fault_sys",
    "avalon_ack_meta", "avalon_ack_sys", "output_ack_meta", "output_ack_sys",
    "heartbeat_meta", "heartbeat_sys", "control_vbl_meta", "control_vbl_sys",
    "control_reset_req_meta", "control_reset_req_sys", "control_reset_out_meta",
    "control_reset_out_sys", "control_pll_lock_meta", "control_pll_lock_sys",
)
AVALON_SYNC_NAMES = (
    "armed_meta", "armed", "request_meta", "request_sync", "route_meta", "route_sync",
    "frame_meta", "frame_sync", "reset_meta", "reset_sync",
)
OUTPUT_SYNC_NAMES = (
    "armed_meta", "armed", "request_meta", "request_sync", "route_meta", "route_sync",
    "direct_meta", "direct_sync", "csync_meta", "csync_sync", "reset_meta", "reset_sync",
    "cfg_meta", "cfg_sync", "pll_meta", "pll_sync",
)
SYNC_NAMES = (
    *(
        f"mister_magik_video_diagnostics_control:magik_video_diagnostics|{name}"
        for name in CONTROL_SYNC_NAMES
    ),
    *(
        f"mister_magik_video_diagnostics_avalon:magik_video_diagnostics_avalon|{name}"
        for name in AVALON_SYNC_NAMES
    ),
    *(
        f"mister_magik_video_diagnostics_output:magik_video_diagnostics_output|{name}"
        for name in OUTPUT_SYNC_NAMES
    ),
)


def quartus_assignment_section(hierarchy: str, names: tuple[str, ...]) -> str:
    return (
        f"; Source assignments for {hierarchy} ;\n"
        + "; Assignment ; Value ; From ; To ;\n"
        + "".join(
            f"; SYNCHRONIZER_IDENTIFICATION ; FORCED_IF_ASYNCHRONOUS ; - ; {name} ;\n"
            for name in names
        )
    )


CONTROL_SYNC_ASSIGNMENTS = quartus_assignment_section(
    "mister_magik_video_diagnostics_control:magik_video_diagnostics",
    CONTROL_SYNC_NAMES,
)
AVALON_SYNC_ASSIGNMENTS = quartus_assignment_section(
    "mister_magik_video_diagnostics_avalon:magik_video_diagnostics_avalon",
    AVALON_SYNC_NAMES,
)
OUTPUT_SYNC_ASSIGNMENTS = quartus_assignment_section(
    "mister_magik_video_diagnostics_output:magik_video_diagnostics_output",
    OUTPUT_SYNC_NAMES,
)
SYNC_ASSIGNMENTS = (
    CONTROL_SYNC_ASSIGNMENTS + AVALON_SYNC_ASSIGNMENTS + OUTPUT_SYNC_ASSIGNMENTS
)
CUSTOM_SYNC = SYNC_ASSIGNMENTS + """\
Info (332114): Report Metastability: Found 30 synchronizer chains.
Info (332114): Fraction of Chains for which MTBFs Could Not be Calculated: 0.233
Info: MagiK diagnostics CDC analysis applied: avalon_payload
Info: MagiK diagnostics CDC analysis applied: output_payload
Info: MagiK diagnostics CDC analysis applied: avalon_route
Info: MagiK diagnostics CDC analysis applied: output_route
Info: MagiK diagnostics CDC analysis applied: fault_trigger
"""

VALID_DIAGNOSTIC_REPORTS = {
    "menu.magik-diagnostic-cdc-skew.rpt": "".join(
        f"; set_max_skew ; 1.{index}00 ; 8.000 ; 7.{index}00 ; from ; to ;\n"
        for index in range(3)
    ),
    "menu.magik-diagnostic-cdc-net-delay.rpt": "".join(
        f"; set_net_delay ; 1.{index}00 ; 8.000 ; 7.{index}00 ; from ; to ; max ;\n"
        for index in range(5)
    ),
    "menu.magik-diagnostic-metastability.rpt": (
        "Report Metastability: Found 30 synchronizer chains.\n"
        + "".join(
            f"; Synchronizer Chain ; {name} ; MTBF 1e+09 years ;\n"
            for name in SYNC_NAMES
        )
    ),
}


def bootstrap_black_warnings(copies: int) -> str:
    return (
        "Warning (332125): Found combinational loop of 6 nodes\n" * copies
        + 'Warning (332126): Node "emu|random|lc0|combout"\n' * copies
        + 'Warning (332126): Node "emu|random|lc0|dataa"\n' * copies
        + 'Warning (332126): Node "emu|random|lc0|datab"\n' * copies
        + 'Warning (332126): Node "emu|random|lc0|datac"\n' * copies
        + 'Warning (332126): Node "emu|random|lc0|datad"\n' * copies
        + 'Warning (332126): Node "emu|random|lc0|datae"\n' * copies
    )


EXPECTED_BOOTSTRAP_BLACK_REMOVED_WARNINGS = bootstrap_black_warnings(7)


class QuartusDeltaTest(unittest.TestCase):
    def run_check(
        self,
        stock: str,
        patched: str,
        baseline: str | None = None,
        fitter_resources: tuple[str, str, str] | None = None,
        diagnostic_reports: dict[str, str] | None = None,
    ) -> tuple[subprocess.CompletedProcess[str], dict[str, object]]:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            stock_path = root / "stock.log"
            baseline_path = root / "baseline.log"
            patched_path = root / "patched.log"
            stock_path.write_text(stock, encoding="utf-8")
            baseline_path.write_text(
                stock if baseline is None else baseline, encoding="utf-8"
            )
            patched_path.write_text(patched, encoding="utf-8")
            command = [
                str(SCRIPT),
                "--stock",
                str(stock_path),
                "--baseline",
                str(baseline_path),
                "--patched",
                str(patched_path),
            ]
            reports = (
                VALID_DIAGNOSTIC_REPORTS
                if diagnostic_reports is None
                else diagnostic_reports
            )
            for name, text in reports.items():
                report_path = root / name
                report_path.write_text(text, encoding="utf-8")
                command.extend(("--patched", str(report_path)))
            if fitter_resources:
                for flavour, resource_text in zip(
                    ("stock", "baseline", "patched"), fitter_resources, strict=True
                ):
                    path = root / f"{flavour}.fit.summary"
                    path.write_text(resource_text, encoding="utf-8")
                    command.extend((f"--{flavour}", str(path)))
            command.append("--json")
            result = subprocess.run(
                command,
                check=False,
                text=True,
                capture_output=True,
            )
            return result, json.loads(result.stdout)

    def test_matching_baseline_and_clean_custom_timing_pass(self) -> None:
        result, payload = self.run_check(BASE, BASE + CUSTOM_SYNC)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(payload["valid"], 1)
        self.assertEqual(payload["invalid_reason"], "ok")

    def test_forced_pll_status_synchronizers_pass(self) -> None:
        patched = CUSTOM_SYNC
        for name in (
            "control_pll_lock_meta",
            "control_pll_lock_sys",
            "pll_meta",
            "pll_sync",
        ):
            patched = patched.replace(
                f"; SYNCHRONIZER_IDENTIFICATION ; FORCED_IF_ASYNCHRONOUS ; - ; {name} ;",
                f"; SYNCHRONIZER_IDENTIFICATION ; FORCED ; - ; {name} ;",
            )
        result, payload = self.run_check(BASE, BASE + patched)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(payload["invalid_reason"], "ok")

    def test_minimum_observer_chain_delta_is_required(self) -> None:
        patched = CUSTOM_SYNC.replace(
            "Found 30 synchronizer chains", "Found 26 synchronizer chains"
        )
        result, payload = self.run_check(BASE, BASE + patched)
        self.assertEqual(result.returncode, 1)
        self.assertIn("custom_synchronizer_chain_count", payload["invalid_reason"])

    def test_new_warning_fails_even_when_warning_code_is_inherited(self) -> None:
        result, payload = self.run_check(BASE, BASE + "Warning (10001): different warning\n" + CUSTOM_SYNC)
        self.assertEqual(result.returncode, 1)
        self.assertIn("warning_added", payload["invalid_reason"])

    def test_missing_inherited_warning_fails_exact_baseline(self) -> None:
        result, payload = self.run_check(BASE, BASE.replace("Warning (10001): inherited warning File: /work/sys/top.v Line: 7\n", "") + CUSTOM_SYNC)
        self.assertEqual(result.returncode, 1)
        self.assertIn("warning_baseline_mismatch", payload["invalid_reason"])

    def test_expected_bootstrap_black_warning_removal_passes(self) -> None:
        stock = BASE + EXPECTED_BOOTSTRAP_BLACK_REMOVED_WARNINGS
        result, payload = self.run_check(stock, BASE + CUSTOM_SYNC)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(payload["expected_bootstrap_black_warning_removal"], True)

    def test_retained_artifact_warning_removal_passes(self) -> None:
        stock = BASE + bootstrap_black_warnings(5)
        result, payload = self.run_check(stock, BASE + CUSTOM_SYNC)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(payload["expected_bootstrap_black_warning_removal"], True)

    def test_consistent_partial_bootstrap_black_warning_removal_fails(self) -> None:
        stock = BASE + bootstrap_black_warnings(7)
        patched = BASE + bootstrap_black_warnings(6) + CUSTOM_SYNC
        result, payload = self.run_check(stock, patched)
        self.assertEqual(result.returncode, 1)
        self.assertIn("warning_baseline_mismatch", payload["invalid_reason"])

    def test_known_removal_count_fails_when_matching_warnings_remain(self) -> None:
        stock = BASE + bootstrap_black_warnings(7)
        patched = BASE + bootstrap_black_warnings(2) + CUSTOM_SYNC
        result, payload = self.run_check(stock, patched)
        self.assertEqual(result.returncode, 1)
        self.assertIn("warning_baseline_mismatch", payload["invalid_reason"])

    def test_arbitrary_proportional_warning_removal_fails(self) -> None:
        stock = BASE + bootstrap_black_warnings(2)
        result, payload = self.run_check(stock, BASE + CUSTOM_SYNC)
        self.assertEqual(result.returncode, 1)
        self.assertIn("warning_baseline_mismatch", payload["invalid_reason"])

    def test_partial_bootstrap_black_warning_removal_fails(self) -> None:
        stock = BASE + EXPECTED_BOOTSTRAP_BLACK_REMOVED_WARNINGS
        patched = BASE + EXPECTED_BOOTSTRAP_BLACK_REMOVED_WARNINGS.replace(
            "Warning (332125): Found combinational loop of 6 nodes\n",
            "",
            1,
        ) + CUSTOM_SYNC
        result, payload = self.run_check(stock, patched)
        self.assertEqual(result.returncode, 1)
        self.assertIn("warning_baseline_mismatch", payload["invalid_reason"])

    def test_unrelated_warning_removal_still_fails_with_expected_removal(self) -> None:
        stock = BASE + EXPECTED_BOOTSTRAP_BLACK_REMOVED_WARNINGS
        patched = BASE.replace(
            "Warning (10001): inherited warning File: /work/sys/top.v Line: 7\n",
            "",
        ) + CUSTOM_SYNC
        result, payload = self.run_check(stock, patched)
        self.assertEqual(result.returncode, 1)
        self.assertIn("warning_baseline_mismatch", payload["invalid_reason"])

    def test_negative_slack_and_nonzero_tns_fail(self) -> None:
        patched = (BASE + CUSTOM_SYNC).replace("setup slack is 0.232", "setup slack is -0.001").replace("0.232               0.000", "-0.001              -0.010")
        result, payload = self.run_check(BASE, patched)
        self.assertEqual(result.returncode, 1)
        self.assertIn("setup_slack_below_minimum", payload["invalid_reason"])
        self.assertIn("tns_nonzero", payload["invalid_reason"])

    def test_new_unconstrained_evidence_fails(self) -> None:
        result, payload = self.run_check(BASE, BASE + "Info: 1 unconstrained output path: magik_route\n" + CUSTOM_SYNC)
        self.assertEqual(result.returncode, 1)
        self.assertIn("unconstrained_added", payload["invalid_reason"])

    def test_more_paths_to_the_same_unconstrained_ports_fails(self) -> None:
        patched = BASE.replace(
            "; Unconstrained Output Port Paths ; 10 ; 10 ;",
            "; Unconstrained Output Port Paths ; 12 ; 12 ;",
        ) + CUSTOM_SYNC
        result, payload = self.run_check(BASE, patched)
        self.assertEqual(result.returncode, 1)
        self.assertIn("unconstrained_output_paths_added", payload["invalid_reason"])

    def test_observer_budgets_are_relative_to_pre_observer_build(self) -> None:
        stock = BASE.replace("setup slack is 0.232", "setup slack is 0.400")
        baseline = stock.replace("setup slack is 0.400", "setup slack is 0.300").replace(
            "; Unconstrained Output Port Paths ; 10 ; 10 ;",
            "; Unconstrained Output Port Paths ; 12 ; 12 ;",
        )
        patched = baseline.replace("setup slack is 0.300", "setup slack is 0.201") + CUSTOM_SYNC
        fitter_resources = (
            "Logic utilization (in ALMs) : 7,000 / 41,910 ( 17 % )\n"
            "Total registers : 20,000\nTotal block memory bits : 1,000,000\nTotal DSP Blocks : 2\n",
            "Logic utilization (in ALMs) : 7,800 / 41,910 ( 19 % )\n"
            "Total registers : 20,500\nTotal block memory bits : 1,000,000\nTotal DSP Blocks : 2\n",
            "Logic utilization (in ALMs) : 8,899 / 41,910 ( 21 % )\n"
            "Total registers : 21,999\nTotal block memory bits : 1,000,000\nTotal DSP Blocks : 2\n",
        )
        result, payload = self.run_check(stock, patched, baseline, fitter_resources)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(payload["baseline_unconstrained_output_paths"], 12)
        self.assertEqual(payload["resource_deltas"]["logic utilization (in alms)"], 1099)
        self.assertEqual(payload["resource_deltas"]["total registers"], 1499)

    def test_alm_budget_excess_fails(self) -> None:
        summaries = (
            "Logic utilization (in ALMs) : 7,000\nTotal registers : 20,000\n"
            "Total block memory bits : 1,000,000\nTotal DSP Blocks : 2\n",
            "Logic utilization (in ALMs) : 7,800\nTotal registers : 20,000\n"
            "Total block memory bits : 1,000,000\nTotal DSP Blocks : 2\n",
            "Logic utilization (in ALMs) : 8,901\nTotal registers : 20,000\n"
            "Total block memory bits : 1,000,000\nTotal DSP Blocks : 2\n",
        )
        result, payload = self.run_check(BASE, BASE + CUSTOM_SYNC, BASE, summaries)
        self.assertEqual(result.returncode, 1)
        self.assertIn("logic_alms_delta", payload["invalid_reason"])

    def test_slack_degradation_excess_fails_above_revised_envelope(self) -> None:
        baseline = BASE.replace("setup slack is 0.232", "setup slack is 0.500")
        patched = (
            baseline.replace("setup slack is 0.500", "setup slack is 0.349")
            + CUSTOM_SYNC
        )
        result, payload = self.run_check(BASE, patched, baseline)
        self.assertEqual(result.returncode, 1)
        self.assertIn("setup_slack_degradation", payload["invalid_reason"])

    def test_missing_baseline_timing_fails(self) -> None:
        baseline = BASE.replace(
            "Info (332146): Worst-case setup slack is 0.232\n", ""
        )
        result, payload = self.run_check(BASE, BASE + CUSTOM_SYNC, baseline)
        self.assertEqual(result.returncode, 1)
        self.assertIn("baseline_setup_slack_missing", payload["invalid_reason"])

    def test_functional_delta_remains_stock_to_final(self) -> None:
        baseline_warning = "Warning (20002): baseline-only observer precursor\n"
        result, _ = self.run_check(BASE, BASE + CUSTOM_SYNC, BASE + baseline_warning)
        self.assertEqual(result.returncode, 0, result.stderr)

        result, payload = self.run_check(
            BASE,
            BASE + baseline_warning + CUSTOM_SYNC,
            BASE + baseline_warning,
        )
        self.assertEqual(result.returncode, 1)
        self.assertIn("warning_added", payload["invalid_reason"])

    def test_resource_and_timing_budgets_are_enforced(self) -> None:
        patched = (BASE + CUSTOM_SYNC).replace(
            "Total logic elements: 10,000", "Total logic elements: 10,801"
        ).replace("Worst-case hold slack is 0.249", "Worst-case hold slack is 0.148")
        result, payload = self.run_check(BASE, patched)
        self.assertEqual(result.returncode, 1)
        self.assertIn("logic_elements_delta", payload["invalid_reason"])
        self.assertIn("hold_slack_below_minimum", payload["invalid_reason"])
        self.assertIn("hold_slack_degradation", payload["invalid_reason"])

    def test_global_mtbf_is_not_custom_chain_evidence(self) -> None:
        patched = BASE + "Info (332114): Worst-Case MTBF of Design is 1e+09 years\n"
        result, payload = self.run_check(BASE, patched)
        self.assertEqual(result.returncode, 1)
        self.assertIn("custom_synchronizer_missing", payload["invalid_reason"])

    def test_named_chain_without_mtbf_fails(self) -> None:
        result, payload = self.run_check(BASE, BASE + SYNC_ASSIGNMENTS)
        self.assertEqual(result.returncode, 1)
        self.assertIn("custom_synchronizer_mtbf_missing", payload["invalid_reason"])

    def test_fraction_change_without_an_extra_calculable_chain_fails(self) -> None:
        patched = BASE.replace(
            "Could Not be Calculated: 0.800",
            "Could Not be Calculated: 0.790",
        ) + SYNC_ASSIGNMENTS
        result, payload = self.run_check(BASE, patched)
        self.assertEqual(result.returncode, 1)
        self.assertIn("custom_synchronizer_mtbf_missing", payload["invalid_reason"])

    def test_missing_diagnostic_report_fails(self) -> None:
        result, payload = self.run_check(BASE, BASE + CUSTOM_SYNC, diagnostic_reports={})
        self.assertEqual(result.returncode, 1)
        self.assertIn("diagnostic_cdc_report_missing", payload["invalid_reason"])

    def test_incomplete_or_negative_cdc_analysis_fails(self) -> None:
        reports = dict(VALID_DIAGNOSTIC_REPORTS)
        reports["menu.magik-diagnostic-cdc-skew.rpt"] = (
            "; set_max_skew ; 1.500 ; 8.000 ; 6.500 ; from ; to ;\n"
            "; set_max_skew path detail Slack 1.500\n"
        )
        result, payload = self.run_check(BASE, BASE + CUSTOM_SYNC, diagnostic_reports=reports)
        self.assertEqual(result.returncode, 1)
        self.assertIn("diagnostic_cdc_analysis_count", payload["invalid_reason"])

        reports = dict(VALID_DIAGNOSTIC_REPORTS)
        reports["menu.magik-diagnostic-cdc-net-delay.rpt"] = reports[
            "menu.magik-diagnostic-cdc-net-delay.rpt"
        ].replace("; 1.000 ;", "; -0.001 ;", 1)
        result, payload = self.run_check(BASE, BASE + CUSTOM_SYNC, diagnostic_reports=reports)
        self.assertEqual(result.returncode, 1)
        self.assertIn("diagnostic_cdc_slack_negative", payload["invalid_reason"])

    def test_synchronizer_hierarchy_is_required(self) -> None:
        wrong = CUSTOM_SYNC.replace(
            "mister_magik_video_diagnostics_avalon:magik_video_diagnostics_avalon",
            "mister_magik_video_diagnostics_avalon:wrong_observer",
            1,
        )
        result, payload = self.run_check(BASE, BASE + wrong)
        self.assertEqual(result.returncode, 1)
        self.assertIn("custom_synchronizer_missing", payload["invalid_reason"])

    def test_output_reset_synchronizer_hierarchy_is_required(self) -> None:
        wrong_output = OUTPUT_SYNC_ASSIGNMENTS.replace(
            "; reset_meta ;", "; unrelated_reset_meta ;", 1
        )
        wrong = CUSTOM_SYNC.replace(OUTPUT_SYNC_ASSIGNMENTS, wrong_output, 1)
        result, payload = self.run_check(BASE, BASE + wrong)
        self.assertEqual(result.returncode, 1)
        self.assertIn("custom_synchronizer_missing", payload["invalid_reason"])

    def test_sdc_uses_quartus_17_register_syntax(self) -> None:
        sdc = (
            SCRIPT.parents[2]
            / "mister/platform/fpga/menu-vblank-latch/mister_magik_video_diagnostics.sdc"
        ).read_text(encoding="utf-8")
        self.assertNotIn("get_registers -nowarn -hierarchical", sdc)
        self.assertIn("get_registers -nowarn -no_duplicates", sdc)


if __name__ == "__main__":
    unittest.main()
