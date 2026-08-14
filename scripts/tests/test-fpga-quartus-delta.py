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
Info (332146): Worst-case setup slack is 0.500
    Info (332119):     Slack       End Point TNS Clock
    Info (332119): ========= =================== =====================
    Info (332119):     0.500               0.000 clk_sys
Info (332146): Worst-case hold slack is 0.249
    Info (332119):     Slack       End Point TNS Clock
    Info (332119): ========= =================== =====================
    Info (332119):     0.249               0.000 clk_sys
Info (332114): Report Metastability: Found 5 synchronizer chains.
Info (332114): Fraction of Chains for which MTBFs Could Not be Calculated: 0.800
; Unconstrained Output Port Paths ; 158 ; 158 ;
Info (332102): Design is not fully constrained for setup requirements
Info (332102): Design is not fully constrained for hold requirements
Total logic elements: 10,000
Total registers: 20,000
Total block memory bits: 1,000,000
Total DSP Blocks: 2
Total PLLs: 3 / 6 ( 50 % )
; PLL Usage Summary ;
; pll_hdmi:pll_hdmi|pll_hdmi_0002:pll_hdmi_inst|altera_pll:altera_pll_i|altera_cyclonev_pll:cyclonev_pll|altera_cyclonev_pll_base:fpll_0|fpll ; ;
; pll_audio:pll_audio|pll_audio_0002:pll_audio_inst|altera_pll:altera_pll_i|general[0].gpll~FRACTIONAL_PLL ; ;
; emu:emu|pll:pll|pll_0002:pll_inst|altera_pll:altera_pll_i|general[0].gpll~FRACTIONAL_PLL ; ;
; AUTO_PARALLEL_SYNTHESIS ; Off ; On ; -- ; -- ;
; NUM_PARALLEL_PROCESSORS ; 4 ; -- ; -- ; -- ;
; PARALLEL_SYNTHESIS ; Off ; On ; -- ; -- ;
Info (20032): Parallel compilation is enabled and will use up to 4 processors
"""

def quartus_assignment_section(hierarchy: str, names: tuple[str, ...]) -> str:
    return (
        f"; Source assignments for {hierarchy} ;\n"
        + "; Assignment ; Value ; From ; To ;\n"
        + "".join(
            f"; SYNCHRONIZER_IDENTIFICATION ; FORCED_IF_ASYNCHRONOUS ; - ; {name} ;\n"
            for name in names
        )
    )


COMPLETION_SYNC_NAMES = (
    "o_readdataack_sync",
    "o_readdataack_sync2",
    "avl_completion_ack_meta",
    "avl_completion_ack_sync",
)
COMPLETION_SYNC_ASSIGNMENTS = quartus_assignment_section(
    "ascal:ascal",
    COMPLETION_SYNC_NAMES,
)
SYNC_NAMES = tuple(
    f"ascal:ascal|{name}"
    for name in COMPLETION_SYNC_NAMES
)
SYNC_ASSIGNMENTS = COMPLETION_SYNC_ASSIGNMENTS
CUSTOM_SYNC = SYNC_ASSIGNMENTS + """\
Info (332114): Report Metastability: Found 6 synchronizer chains.
Info (332114): Fraction of Chains for which MTBFs Could Not be Calculated: 0.666667
Info: MagiK diagnostics CDC analysis applied: scaler_completion_request_ack
"""

VALID_DIAGNOSTIC_REPORTS = {
    "menu.magik-diagnostic-cdc-skew.rpt": "No paths to report.\n",
    "menu.magik-diagnostic-cdc-net-delay.rpt": (
        "; set_net_delay ; 1.250 ; 10.000 ; 8.750 ; sources ; destinations ; max ;\n"
        "; set_net_delay ; 1.150 ; 10.000 ; 8.850 ; sources ; destinations ; max ;\n"
        "; -- ; 1.500 ; 10.000 ; 8.500 ; ascal:ascal|avl_readdataack ; "
        "ascal:ascal|o_readdataack_sync ; max ;\n"
        "; -- ; 1.250 ; 10.000 ; 8.750 ; ascal:ascal|o_readdataack_sync2 ; "
        "ascal:ascal|avl_completion_ack_meta ; max ;\n"
    ),
    "menu.magik-diagnostic-metastability.rpt": (
        "Report Metastability: Found 40 synchronizer chains.\n"
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

    def test_quartus_policy_mismatch_fails(self) -> None:
        patched = (BASE + CUSTOM_SYNC).replace(
            "; NUM_PARALLEL_PROCESSORS ; 4 ;",
            "; NUM_PARALLEL_PROCESSORS ; 9 ;",
        )
        result, payload = self.run_check(BASE, patched)
        self.assertEqual(result.returncode, 1)
        self.assertIn("quartus_policy_mismatch", payload["invalid_reason"])

    def test_quartus_processor_use_mismatch_fails(self) -> None:
        patched = (BASE + CUSTOM_SYNC).replace(
            "will use up to 4 processors",
            "will use up to 9 processors",
        )
        result, payload = self.run_check(BASE, patched)
        self.assertEqual(result.returncode, 1)
        self.assertIn("quartus_processor_use_mismatch", payload["invalid_reason"])

    def test_unrelated_total_chain_drift_fails(self) -> None:
        patched = CUSTOM_SYNC.replace(
            "Found 6 synchronizer chains", "Found 10 synchronizer chains"
        ).replace(
            "Could Not be Calculated: 0.666667",
            "Could Not be Calculated: 0.800",
        )
        result, payload = self.run_check(BASE, BASE + patched)
        self.assertEqual(result.returncode, 1)
        self.assertIn("synchronizer_chain_count_mismatch", payload["invalid_reason"])
        self.assertEqual(payload["baseline_synchronizer_chains"], 5)
        self.assertEqual(payload["patched_synchronizer_chains"], 10)
        self.assertEqual(payload["baseline_calculable_synchronizer_chains"], 1)
        self.assertEqual(payload["patched_calculable_synchronizer_chains"], 2)

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
        patched = (BASE + CUSTOM_SYNC).replace("setup slack is 0.500", "setup slack is -0.001").replace("0.500               0.000", "-0.001              -0.010")
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
            "; Unconstrained Output Port Paths ; 158 ; 158 ;",
            "; Unconstrained Output Port Paths ; 12 ; 12 ;",
        ) + CUSTOM_SYNC
        result, payload = self.run_check(BASE, patched)
        self.assertEqual(result.returncode, 1)
        self.assertIn("unconstrained_output_paths_mismatch", payload["invalid_reason"])

    def test_fewer_paths_to_the_same_unconstrained_ports_fails(self) -> None:
        patched = BASE.replace(
            "; Unconstrained Output Port Paths ; 158 ; 158 ;",
            "; Unconstrained Output Port Paths ; 8 ; 8 ;",
        ) + CUSTOM_SYNC
        result, payload = self.run_check(BASE, patched)
        self.assertEqual(result.returncode, 1)
        self.assertIn("unconstrained_output_paths_mismatch", payload["invalid_reason"])

    def test_repair_budgets_are_relative_to_pre_observer_build(self) -> None:
        stock = BASE.replace("setup slack is 0.500", "setup slack is 0.600")
        baseline = stock.replace("setup slack is 0.400", "setup slack is 0.300").replace(
            "; Unconstrained Output Port Paths ; 158 ; 158 ;",
            "; Unconstrained Output Port Paths ; 158 ; 158 ;",
        )
        patched = baseline.replace("setup slack is 0.600", "setup slack is 0.451") + CUSTOM_SYNC
        fitter_resources = (
            "Logic utilization (in ALMs) : 7,000 / 41,910 ( 17 % )\n"
            "Total registers : 20,000\nTotal block memory bits : 1,000,000\nTotal DSP Blocks : 2\nTotal PLLs : 3 / 6\n",
            "Logic utilization (in ALMs) : 7,800 / 41,910 ( 19 % )\n"
            "Total registers : 20,500\nTotal block memory bits : 1,000,000\nTotal DSP Blocks : 2\nTotal PLLs : 3 / 6\n",
            "Logic utilization (in ALMs) : 7,950 / 41,910 ( 19 % )\n"
            "Total registers : 20,596\nTotal block memory bits : 1,000,000\nTotal DSP Blocks : 2\nTotal PLLs : 3 / 6\n",
        )
        result, payload = self.run_check(stock, patched, baseline, fitter_resources)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(payload["baseline_unconstrained_output_paths"], 158)
        self.assertEqual(payload["resource_deltas"]["logic utilization (in alms)"], 150)
        self.assertEqual(payload["resource_deltas"]["total registers"], 96)

    def test_alm_budget_excess_fails(self) -> None:
        summaries = (
            "Logic utilization (in ALMs) : 7,000\nTotal registers : 20,000\n"
            "Total block memory bits : 1,000,000\nTotal DSP Blocks : 2\nTotal PLLs : 3 / 6\n",
            "Logic utilization (in ALMs) : 7,800\nTotal registers : 20,000\n"
            "Total block memory bits : 1,000,000\nTotal DSP Blocks : 2\nTotal PLLs : 3 / 6\n",
            "Logic utilization (in ALMs) : 7,951\nTotal registers : 20,000\n"
            "Total block memory bits : 1,000,000\nTotal DSP Blocks : 2\nTotal PLLs : 3 / 6\n",
        )
        result, payload = self.run_check(BASE, BASE + CUSTOM_SYNC, BASE, summaries)
        self.assertEqual(result.returncode, 1)
        self.assertIn("logic_alms_delta", payload["invalid_reason"])

    def test_register_budget_excess_fails(self) -> None:
        summaries = (
            "Logic utilization (in ALMs) : 7,000\nTotal registers : 20,000\n"
            "Total block memory bits : 1,000,000\nTotal DSP Blocks : 2\nTotal PLLs : 3 / 6\n",
            "Logic utilization (in ALMs) : 7,800\nTotal registers : 20,500\n"
            "Total block memory bits : 1,000,000\nTotal DSP Blocks : 2\nTotal PLLs : 3 / 6\n",
            "Logic utilization (in ALMs) : 7,800\nTotal registers : 20,597\n"
            "Total block memory bits : 1,000,000\nTotal DSP Blocks : 2\nTotal PLLs : 3 / 6\n",
        )
        result, payload = self.run_check(BASE, BASE + CUSTOM_SYNC, BASE, summaries)
        self.assertEqual(result.returncode, 1)
        self.assertIn("registers_delta", payload["invalid_reason"])

    def test_slack_degradation_excess_fails_above_revised_envelope(self) -> None:
        baseline = BASE.replace("setup slack is 0.500", "setup slack is 0.600")
        patched = (
            baseline.replace("setup slack is 0.600", "setup slack is 0.449")
            + CUSTOM_SYNC
        )
        result, payload = self.run_check(BASE, patched, baseline)
        self.assertEqual(result.returncode, 1)
        self.assertIn("setup_slack_degradation", payload["invalid_reason"])

    def test_missing_baseline_timing_fails(self) -> None:
        baseline = BASE.replace(
            "Info (332146): Worst-case setup slack is 0.500\n", ""
        )
        result, payload = self.run_check(BASE, BASE + CUSTOM_SYNC, baseline)
        self.assertEqual(result.returncode, 1)
        self.assertIn("baseline_setup_slack_missing", payload["invalid_reason"])

    def test_functional_delta_remains_stock_to_final(self) -> None:
        baseline_warning = "Warning (20002): baseline-only repair precursor\n"
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
        ).replace("Worst-case hold slack is 0.249", "Worst-case hold slack is 0.098")
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
        reports["menu.magik-diagnostic-cdc-net-delay.rpt"] = (
            "; set_net_delay ; 1.500 ; 8.000 ; 6.500 ; from ; to ;\n"
        )
        result, payload = self.run_check(BASE, BASE + CUSTOM_SYNC, diagnostic_reports=reports)
        self.assertEqual(result.returncode, 1)
        self.assertIn("diagnostic_cdc_analysis_count", payload["invalid_reason"])

    def test_duplicate_forward_net_delay_path_fails(self) -> None:
        reports = dict(VALID_DIAGNOSTIC_REPORTS)
        reports["menu.magik-diagnostic-cdc-net-delay.rpt"] = reports[
            "menu.magik-diagnostic-cdc-net-delay.rpt"
        ].replace(
            "ascal:ascal|o_readdataack_sync2 ; ascal:ascal|avl_completion_ack_meta",
            "ascal:ascal|avl_readdataack ; ascal:ascal|o_readdataack_sync",
        )
        result, payload = self.run_check(
            BASE, BASE + CUSTOM_SYNC, diagnostic_reports=reports
        )
        self.assertEqual(result.returncode, 1)
        self.assertIn(
            "diagnostic_cdc_path_identity_mismatch", payload["invalid_reason"]
        )

    def test_duplicate_reverse_net_delay_path_fails(self) -> None:
        reports = dict(VALID_DIAGNOSTIC_REPORTS)
        reports["menu.magik-diagnostic-cdc-net-delay.rpt"] = reports[
            "menu.magik-diagnostic-cdc-net-delay.rpt"
        ].replace(
            "ascal:ascal|avl_readdataack ; ascal:ascal|o_readdataack_sync",
            "ascal:ascal|o_readdataack_sync2 ; ascal:ascal|avl_completion_ack_meta",
        )
        result, payload = self.run_check(
            BASE, BASE + CUSTOM_SYNC, diagnostic_reports=reports
        )
        self.assertEqual(result.returncode, 1)
        self.assertIn(
            "diagnostic_cdc_path_identity_mismatch", payload["invalid_reason"]
        )

    def test_pll_count_drift_fails(self) -> None:
        patched = (BASE + CUSTOM_SYNC).replace(
            "Total PLLs: 3 / 6", "Total PLLs: 4 / 6"
        )
        result, payload = self.run_check(BASE, patched)
        self.assertEqual(result.returncode, 1)
        self.assertIn("pll_count_mismatch", payload["invalid_reason"])
        self.assertIn("pll_identity_count_mismatch", payload["invalid_reason"])

    def test_pll_identity_drift_fails(self) -> None:
        patched = (BASE + CUSTOM_SYNC).replace(
            "pll_audio:pll_audio|pll_audio_0002:pll_audio_inst",
            "pll_extra:pll_extra|pll_extra_0002:pll_extra_inst",
        )
        result, payload = self.run_check(BASE, patched)
        self.assertEqual(result.returncode, 1)
        self.assertIn("pll_identity_mismatch", payload["invalid_reason"])

    def test_completion_chain_mtbf_below_device_hour_gate_fails(self) -> None:
        reports = dict(VALID_DIAGNOSTIC_REPORTS)
        reports["menu.magik-diagnostic-metastability.rpt"] = reports[
            "menu.magik-diagnostic-metastability.rpt"
        ].replace("1e+09 years", "1e+08 years", 1)
        result, payload = self.run_check(
            BASE, BASE + CUSTOM_SYNC, diagnostic_reports=reports
        )
        self.assertEqual(result.returncode, 1)
        self.assertIn(
            "diagnostic_metastability_mtbf_below_minimum",
            payload["invalid_reason"],
        )

    def test_synchronizer_hierarchy_is_required(self) -> None:
        wrong = CUSTOM_SYNC.replace(
            "ascal:ascal",
            "ascal:wrong_ascal",
            1,
        )
        result, payload = self.run_check(BASE, BASE + wrong)
        self.assertEqual(result.returncode, 1)
        self.assertIn("custom_synchronizer_missing", payload["invalid_reason"])

    def test_second_completion_synchronizer_stage_is_required(self) -> None:
        wrong = CUSTOM_SYNC.replace(
            "; o_readdataack_sync2 ;", "; unrelated_completion_sync ;", 1
        )
        result, payload = self.run_check(BASE, BASE + wrong)
        self.assertEqual(result.returncode, 1)
        self.assertIn("custom_synchronizer_missing", payload["invalid_reason"])

    def test_sdc_uses_quartus_17_register_syntax(self) -> None:
        sdc = (
            SCRIPT.parents[2]
            / "mister/platform/fpga/menu-vblank-latch/mister_magik_video_diagnostics.sdc"
        ).read_text(encoding="utf-8")
        self.assertIn("get_registers -nowarn -no_duplicates", sdc)
        self.assertEqual(sdc.count("set_net_delay -max 10.0"), 2)
        self.assertNotIn("set_max_skew", sdc)
        self.assertNotIn("set_false_path", sdc)


if __name__ == "__main__":
    unittest.main()
