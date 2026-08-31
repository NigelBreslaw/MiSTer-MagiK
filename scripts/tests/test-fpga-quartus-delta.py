#!/usr/bin/env python3
# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

"""Fixture tests for check-fpga-quartus-delta.py."""

from __future__ import annotations

import json
import subprocess
import tempfile
import unittest
from pathlib import Path

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
SYNC_NAMES = tuple(f"ascal:ascal|{name}" for name in COMPLETION_SYNC_NAMES)
SYNC_ASSIGNMENTS = COMPLETION_SYNC_ASSIGNMENTS
CUSTOM_SYNC = (
    SYNC_ASSIGNMENTS
    + """\
Info (332114): Report Metastability: Found 7 synchronizer chains.
Info (332114): Fraction of Chains for which MTBFs Could Not be Calculated: 0.571429
Info: MagiK diagnostics CDC analysis applied: scaler_completion_request_ack
"""
)
RAW_SCALER_SYNC_ASSIGNMENTS = quartus_assignment_section(
    "mister_magik_raw_scaler_ordered_frame:magik_raw_scaler_ordered_frame",
    ("generation_meta", "generation_sync"),
)
EXPERIMENTAL_CUSTOM_SYNC = (
    SYNC_ASSIGNMENTS
    + RAW_SCALER_SYNC_ASSIGNMENTS
    + """\
Info (332114): Report Metastability: Found 8 synchronizer chains.
Info (332114): Fraction of Chains for which MTBFs Could Not be Calculated: 0.500000
Info: MagiK diagnostics CDC analysis applied: scaler_completion_request_ack
"""
)
SCALER_FETCH_SYNC_ASSIGNMENTS = quartus_assignment_section(
    "mister_magik_scaler_fetch_liveness_state:magik_scaler_fetch_liveness_state",
    (
        "record_ready_meta",
        "record_ready_sync",
        "reset_meta",
        "reset_sync",
    ),
)
SCALER_SNAPSHOT_SOURCE_SYNC_ASSIGNMENTS = quartus_assignment_section(
    "mister_magik_scaler_scheduler_snapshot:scheduler_snapshot",
    ("request_meta", "request_sync"),
)
SCALER_SNAPSHOT_DESTINATION_SYNC_ASSIGNMENTS = quartus_assignment_section(
    "mister_magik_scaler_fetch_liveness_state:magik_scaler_fetch_liveness_state",
    ("snapshot_response_meta", "snapshot_response_sync"),
)
SCALER_FETCH_CUSTOM_SYNC = (
    SYNC_ASSIGNMENTS
    + SCALER_FETCH_SYNC_ASSIGNMENTS
    + SCALER_SNAPSHOT_SOURCE_SYNC_ASSIGNMENTS
    + SCALER_SNAPSHOT_DESTINATION_SYNC_ASSIGNMENTS
    + """\
Info (332114): Report Metastability: Found 11 synchronizer chains.
Info (332114): Fraction of Chains for which MTBFs Could Not be Calculated: 0.363636
Info: MagiK diagnostics CDC analysis applied: scaler_completion_request_ack
"""
)


def metastability_chain(
    index: int, source: str, synchronization_node: str, registers: tuple[str, ...]
) -> str:
    return (
        f"Synchronizer Chain #{index}: Worst-Case MTBF is Greater than 10 Billion Years\n"
        f"; Source Node ; {source} ;\n"
        f"; Synchronization Node ; {synchronization_node} ;\n"
        "; Worst-Case MTBF (years) ; Greater than 10 Billion ;\n"
        "; Synchronization Registers ; ;\n"
        + "".join(f"; {register} ; ;\n" for register in registers)
    )


METASTABILITY_CHAINS = [
    (
        "ascal:ascal|avl_readdataack",
        "ascal:ascal|o_readdataack_sync",
        ("ascal:ascal|o_readdataack_sync",),
    ),
    (
        "ascal:ascal|o_readdataack_sync2",
        "ascal:ascal|avl_completion_ack_meta",
        ("ascal:ascal|avl_completion_ack_meta", "ascal:ascal|avl_completion_ack_sync"),
    ),
]


def net_delay_detail(source: str, target: str) -> str:
    return f"; -- ; 1.000 ; 10.000 ; 9.000 ; {source} ; {target} ; max ;\n"


VALID_DIAGNOSTIC_REPORTS = {
    "menu.magik-diagnostic-cdc-skew.rpt": "No paths to report.\n",
    "menu.magik-diagnostic-cdc-net-delay.rpt": (
        "; set_net_delay ; 1.250 ; 10.000 ; 8.750 ; sources ; destinations ; max ;\n"
        "; set_net_delay ; 1.150 ; 10.000 ; 8.850 ; sources ; destinations ; max ;\n"
        + net_delay_detail(
            "ascal:ascal|avl_readdataack", "ascal:ascal|o_readdataack_sync"
        )
        + net_delay_detail(
            "ascal:ascal|o_readdataack_sync2~DUPLICATE",
            "ascal:ascal|avl_completion_ack_meta",
        )
    ),
    "menu.magik-diagnostic-metastability.rpt": (
        "Report Metastability: Found 40 synchronizer chains.\n"
        + "".join(
            metastability_chain(index, source, node, registers)
            for index, (source, node, registers) in enumerate(METASTABILITY_CHAINS, 1)
        )
    ),
}
EXPERIMENTAL_DIAGNOSTIC_REPORTS = {
    **VALID_DIAGNOSTIC_REPORTS,
    "menu.magik-diagnostic-cdc-net-delay.rpt": (
        VALID_DIAGNOSTIC_REPORTS["menu.magik-diagnostic-cdc-net-delay.rpt"]
        + "; set_net_delay ; 1.050 ; 10.000 ; 8.950 ; sources ; destinations ; max ;\n"
        + net_delay_detail(
            "mister_magik_raw_scaler_ordered_frame:magik_raw_scaler_ordered_frame|source_generation",
            "mister_magik_raw_scaler_ordered_frame:magik_raw_scaler_ordered_frame|generation_meta",
        )
    ),
    "menu.magik-diagnostic-metastability.rpt": (
        VALID_DIAGNOSTIC_REPORTS["menu.magik-diagnostic-metastability.rpt"]
        + metastability_chain(
            3,
            "mister_magik_raw_scaler_ordered_frame:magik_raw_scaler_ordered_frame|source_generation",
            "mister_magik_raw_scaler_ordered_frame:magik_raw_scaler_ordered_frame|generation_meta",
            (
                "mister_magik_raw_scaler_ordered_frame:magik_raw_scaler_ordered_frame|generation_meta",
                "mister_magik_raw_scaler_ordered_frame:magik_raw_scaler_ordered_frame|generation_sync",
            ),
        )
    ),
}
SCALER_FETCH_DIAGNOSTIC_REPORTS = {
    **VALID_DIAGNOSTIC_REPORTS,
    "menu.magik-diagnostic-cdc-net-delay.rpt": (
        VALID_DIAGNOSTIC_REPORTS["menu.magik-diagnostic-cdc-net-delay.rpt"]
        + "; set_net_delay ; 1.050 ; 10.000 ; 8.950 ; sources ; destinations ; max ;\n"
        + net_delay_detail(
            "mister_magik_scaler_fetch_liveness_state:magik_scaler_fetch_liveness_state|record_ready",
            "mister_magik_scaler_fetch_liveness_state:magik_scaler_fetch_liveness_state|record_ready_meta",
        )
        + "; set_net_delay ; 1.015 ; 10.000 ; 8.985 ; sources ; destinations ; max ;\n"
        + net_delay_detail(
            "mister_magik_scaler_fetch_liveness_state:magik_scaler_fetch_liveness_state|snapshot_request_toggle",
            "mister_magik_scaler_scheduler_snapshot:scheduler_snapshot|request_meta",
        )
        + "; set_net_delay ; 1.010 ; 10.000 ; 8.990 ; sources ; destinations ; max ;\n"
        + net_delay_detail(
            "mister_magik_scaler_scheduler_snapshot:scheduler_snapshot|response_handoff_bit",
            "mister_magik_scaler_fetch_liveness_state:magik_scaler_fetch_liveness_state|snapshot_response_meta",
        )
        + "; set_net_delay ; 1.005 ; 10.000 ; 8.995 ; sources ; destinations ; max ;\n"
        + "".join(
            net_delay_detail(
                f"mister_magik_scaler_scheduler_snapshot:scheduler_snapshot|compact_evidence[{bit}]",
                f"mister_magik_scaler_fetch_liveness_state:magik_scaler_fetch_liveness_state|scheduler_snapshot_capture[{bit}]",
            )
            for bit in range(6)
        )
    ),
    "menu.magik-diagnostic-metastability.rpt": (
        VALID_DIAGNOSTIC_REPORTS["menu.magik-diagnostic-metastability.rpt"]
        + metastability_chain(
            3,
            "mister_magik_scaler_fetch_liveness_state:magik_scaler_fetch_liveness_state|record_ready",
            "mister_magik_scaler_fetch_liveness_state:magik_scaler_fetch_liveness_state|record_ready_meta",
            (
                "mister_magik_scaler_fetch_liveness_state:magik_scaler_fetch_liveness_state|record_ready_meta",
                "mister_magik_scaler_fetch_liveness_state:magik_scaler_fetch_liveness_state|record_ready_sync",
            ),
        )
        + metastability_chain(
            4,
            "reset_req",
            "mister_magik_scaler_fetch_liveness_state:magik_scaler_fetch_liveness_state|reset_meta",
            (
                "mister_magik_scaler_fetch_liveness_state:magik_scaler_fetch_liveness_state|reset_meta",
                "mister_magik_scaler_fetch_liveness_state:magik_scaler_fetch_liveness_state|reset_sync",
            ),
        )
        + metastability_chain(
            5,
            "mister_magik_scaler_fetch_liveness_state:magik_scaler_fetch_liveness_state|snapshot_request_toggle",
            "mister_magik_scaler_fetch_liveness_state:magik_scaler_fetch_liveness_state|mister_magik_scaler_scheduler_snapshot:scheduler_snapshot|request_meta",
            (
                "mister_magik_scaler_fetch_liveness_state:magik_scaler_fetch_liveness_state|mister_magik_scaler_scheduler_snapshot:scheduler_snapshot|request_meta",
                "mister_magik_scaler_fetch_liveness_state:magik_scaler_fetch_liveness_state|mister_magik_scaler_scheduler_snapshot:scheduler_snapshot|request_sync",
            ),
        )
        + metastability_chain(
            6,
            "mister_magik_scaler_fetch_liveness_state:magik_scaler_fetch_liveness_state|mister_magik_scaler_scheduler_snapshot:scheduler_snapshot|response_handoff_bit",
            "mister_magik_scaler_fetch_liveness_state:magik_scaler_fetch_liveness_state|snapshot_response_meta",
            (
                "mister_magik_scaler_fetch_liveness_state:magik_scaler_fetch_liveness_state|snapshot_response_meta",
                "mister_magik_scaler_fetch_liveness_state:magik_scaler_fetch_liveness_state|snapshot_response_sync",
            ),
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
        experimental_diagnostic: bool = False,
        experimental_scaler_fetch: bool = False,
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
            if experimental_diagnostic:
                command.append("--experimental-diagnostic")
            if experimental_scaler_fetch:
                command.append("--experimental-scaler-fetch")
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
        detailed = payload["diagnostic_cdc_detailed_path_counts"]
        report = "menu.magik-diagnostic-cdc-net-delay.rpt"
        self.assertEqual(detailed[report], 2)
        self.assertEqual(detailed[f"{report}:completion_request"], 1)
        self.assertEqual(detailed[f"{report}:completion_ack"], 1)
        self.assertEqual(
            set(payload["diagnostic_metastability_mtbf_years"]),
            {
                "completion_request",
                "completion_ack",
            },
        )
        self.assertGreaterEqual(
            payload["diagnostic_metastability_combined_mtbf_years"],
            payload["minimum_custom_mtbf_device_hours"] / (24.0 * 365.25),
        )

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
            "Found 7 synchronizer chains", "Found 8 synchronizer chains"
        ).replace(
            "Could Not be Calculated: 0.571429",
            "Could Not be Calculated: 0.625000",
        )
        result, payload = self.run_check(BASE, BASE + patched)
        self.assertEqual(result.returncode, 1)
        self.assertIn("synchronizer_chain_count_mismatch", payload["invalid_reason"])
        self.assertEqual(payload["baseline_synchronizer_chains"], 5)
        self.assertEqual(payload["patched_synchronizer_chains"], 8)
        self.assertEqual(payload["baseline_calculable_synchronizer_chains"], 1)
        self.assertEqual(payload["patched_calculable_synchronizer_chains"], 3)

    def test_experimental_profile_accepts_bounded_timing_and_aggregate_chain_drift(
        self,
    ) -> None:
        baseline = BASE.replace("setup slack is 0.500", "setup slack is 0.660").replace(
            "0.500               0.000", "0.660               0.000"
        )
        patched = baseline.replace(
            "setup slack is 0.660", "setup slack is 0.389"
        ).replace(
            "0.660               0.000", "0.389               0.000"
        ) + EXPERIMENTAL_CUSTOM_SYNC.replace(
            "Found 8 synchronizer chains", "Found 6 synchronizer chains"
        ).replace(
            "Could Not be Calculated: 0.500000",
            "Could Not be Calculated: 0.333333",
        )
        result, payload = self.run_check(
            BASE,
            patched,
            baseline,
            diagnostic_reports=EXPERIMENTAL_DIAGNOSTIC_REPORTS,
            experimental_diagnostic=True,
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(payload["signoff_profile"], "experimental_raw_scaler")

        result, payload = self.run_check(BASE, patched, baseline)
        self.assertEqual(result.returncode, 1)
        self.assertIn("setup_slack_below_minimum", payload["invalid_reason"])
        self.assertIn("setup_slack_degradation", payload["invalid_reason"])
        self.assertIn("synchronizer_chain_count_mismatch", payload["invalid_reason"])

    def test_scaler_fetch_profile_accepts_exact_hierarchy_and_canonical_paths(
        self,
    ) -> None:
        baseline = BASE.replace("setup slack is 0.500", "setup slack is 0.660").replace(
            "0.500               0.000", "0.660               0.000"
        )
        patched = (
            baseline.replace("setup slack is 0.660", "setup slack is 0.389").replace(
                "0.660               0.000", "0.389               0.000"
            )
            + SCALER_FETCH_CUSTOM_SYNC
        )
        result, payload = self.run_check(
            BASE,
            patched,
            baseline,
            diagnostic_reports=SCALER_FETCH_DIAGNOSTIC_REPORTS,
            experimental_scaler_fetch=True,
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(payload["signoff_profile"], "experimental_scaler_fetch")
        self.assertEqual(payload["patched_unconstrained_output_paths"], 158)

    def test_experimental_profile_has_explicit_resource_ceiling(self) -> None:
        resources = (
            (
                "Logic utilization (in ALMs) : 7,000\nTotal registers : 20,000\n"
                "Total block memory bits : 1,000,000\nTotal DSP Blocks : 2\nTotal PLLs : 3 / 6\n"
            ),
            (
                "Logic utilization (in ALMs) : 7,800\nTotal registers : 20,500\n"
                "Total block memory bits : 1,000,000\nTotal DSP Blocks : 2\nTotal PLLs : 3 / 6\n"
            ),
            (
                "Logic utilization (in ALMs) : 8,008\nTotal registers : 20,724\n"
                "Total block memory bits : 1,000,000\nTotal DSP Blocks : 2\nTotal PLLs : 3 / 6\n"
            ),
        )
        result, payload = self.run_check(
            BASE,
            BASE + EXPERIMENTAL_CUSTOM_SYNC,
            BASE,
            resources,
            diagnostic_reports=EXPERIMENTAL_DIAGNOSTIC_REPORTS,
            experimental_diagnostic=True,
        )
        self.assertEqual(result.returncode, 0, result.stderr)

        result, payload = self.run_check(BASE, BASE + CUSTOM_SYNC, BASE, resources)
        self.assertEqual(result.returncode, 1)
        self.assertIn("logic_alms_delta", payload["invalid_reason"])
        self.assertIn("registers_delta", payload["invalid_reason"])

    def test_new_warning_fails_even_when_warning_code_is_inherited(self) -> None:
        result, payload = self.run_check(
            BASE, BASE + "Warning (10001): different warning\n" + CUSTOM_SYNC
        )
        self.assertEqual(result.returncode, 1)
        self.assertIn("warning_added", payload["invalid_reason"])

    def test_missing_inherited_warning_fails_exact_baseline(self) -> None:
        result, payload = self.run_check(
            BASE,
            BASE.replace(
                "Warning (10001): inherited warning File: /work/sys/top.v Line: 7\n", ""
            )
            + CUSTOM_SYNC,
        )
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
        patched = (
            BASE
            + EXPECTED_BOOTSTRAP_BLACK_REMOVED_WARNINGS.replace(
                "Warning (332125): Found combinational loop of 6 nodes\n",
                "",
                1,
            )
            + CUSTOM_SYNC
        )
        result, payload = self.run_check(stock, patched)
        self.assertEqual(result.returncode, 1)
        self.assertIn("warning_baseline_mismatch", payload["invalid_reason"])

    def test_unrelated_warning_removal_still_fails_with_expected_removal(self) -> None:
        stock = BASE + EXPECTED_BOOTSTRAP_BLACK_REMOVED_WARNINGS
        patched = (
            BASE.replace(
                "Warning (10001): inherited warning File: /work/sys/top.v Line: 7\n",
                "",
            )
            + CUSTOM_SYNC
        )
        result, payload = self.run_check(stock, patched)
        self.assertEqual(result.returncode, 1)
        self.assertIn("warning_baseline_mismatch", payload["invalid_reason"])

    def test_negative_slack_and_nonzero_tns_fail(self) -> None:
        patched = (
            (BASE + CUSTOM_SYNC)
            .replace("setup slack is 0.500", "setup slack is -0.001")
            .replace("0.500               0.000", "-0.001              -0.010")
        )
        result, payload = self.run_check(BASE, patched)
        self.assertEqual(result.returncode, 1)
        self.assertIn("setup_slack_below_minimum", payload["invalid_reason"])
        self.assertIn("tns_nonzero", payload["invalid_reason"])

    def test_new_unconstrained_evidence_fails(self) -> None:
        result, payload = self.run_check(
            BASE,
            BASE + "Info: 1 unconstrained output path: magik_route\n" + CUSTOM_SYNC,
        )
        self.assertEqual(result.returncode, 1)
        self.assertIn("unconstrained_added", payload["invalid_reason"])

    def test_more_paths_to_the_same_unconstrained_ports_fails(self) -> None:
        patched = (
            BASE.replace(
                "; Unconstrained Output Port Paths ; 158 ; 158 ;",
                "; Unconstrained Output Port Paths ; 12 ; 12 ;",
            )
            + CUSTOM_SYNC
        )
        result, payload = self.run_check(BASE, patched)
        self.assertEqual(result.returncode, 1)
        self.assertIn("unconstrained_output_paths_mismatch", payload["invalid_reason"])

    def test_fewer_paths_to_the_same_unconstrained_ports_fails(self) -> None:
        patched = (
            BASE.replace(
                "; Unconstrained Output Port Paths ; 158 ; 158 ;",
                "; Unconstrained Output Port Paths ; 8 ; 8 ;",
            )
            + CUSTOM_SYNC
        )
        result, payload = self.run_check(BASE, patched)
        self.assertEqual(result.returncode, 1)
        self.assertIn("unconstrained_output_paths_mismatch", payload["invalid_reason"])

    def test_diagnostic_two_path_exception_is_explicit(self) -> None:
        patched = (
            BASE.replace(
                "; Unconstrained Output Port Paths ; 158 ; 158 ;",
                "; Unconstrained Output Port Paths ; 160 ; 160 ;",
            )
            + EXPERIMENTAL_CUSTOM_SYNC
        )
        result, payload = self.run_check(
            BASE,
            patched,
            diagnostic_reports=EXPERIMENTAL_DIAGNOSTIC_REPORTS,
            experimental_diagnostic=True,
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertTrue(payload["diagnostic_unconstrained_output_paths_exception"])

    def test_repair_budgets_are_relative_to_pre_observer_build(self) -> None:
        stock = BASE.replace("setup slack is 0.500", "setup slack is 0.600")
        baseline = stock.replace(
            "setup slack is 0.400", "setup slack is 0.300"
        ).replace(
            "; Unconstrained Output Port Paths ; 158 ; 158 ;",
            "; Unconstrained Output Port Paths ; 158 ; 158 ;",
        )
        patched = (
            baseline.replace("setup slack is 0.600", "setup slack is 0.451")
            + CUSTOM_SYNC
        )
        fitter_resources = (
            (
                "Logic utilization (in ALMs) : 7,000 / 41,910 ( 17 % )\n"
                "Total registers : 20,000\nTotal block memory bits : 1,000,000\nTotal DSP Blocks : 2\nTotal PLLs : 3 / 6\n"
            ),
            (
                "Logic utilization (in ALMs) : 7,800 / 41,910 ( 19 % )\n"
                "Total registers : 20,500\nTotal block memory bits : 1,000,000\nTotal DSP Blocks : 2\nTotal PLLs : 3 / 6\n"
            ),
            (
                "Logic utilization (in ALMs) : 7,950 / 41,910 ( 19 % )\n"
                "Total registers : 20,596\nTotal block memory bits : 1,000,000\nTotal DSP Blocks : 2\nTotal PLLs : 3 / 6\n"
            ),
        )
        result, payload = self.run_check(stock, patched, baseline, fitter_resources)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(payload["baseline_unconstrained_output_paths"], 158)
        self.assertEqual(payload["resource_deltas"]["logic utilization (in alms)"], 150)
        self.assertEqual(payload["resource_deltas"]["total registers"], 96)

    def test_alm_budget_excess_fails(self) -> None:
        summaries = (
            (
                "Logic utilization (in ALMs) : 7,000\nTotal registers : 20,000\n"
                "Total block memory bits : 1,000,000\nTotal DSP Blocks : 2\nTotal PLLs : 3 / 6\n"
            ),
            (
                "Logic utilization (in ALMs) : 7,800\nTotal registers : 20,000\n"
                "Total block memory bits : 1,000,000\nTotal DSP Blocks : 2\nTotal PLLs : 3 / 6\n"
            ),
            (
                "Logic utilization (in ALMs) : 7,951\nTotal registers : 20,000\n"
                "Total block memory bits : 1,000,000\nTotal DSP Blocks : 2\nTotal PLLs : 3 / 6\n"
            ),
        )
        result, payload = self.run_check(BASE, BASE + CUSTOM_SYNC, BASE, summaries)
        self.assertEqual(result.returncode, 1)
        self.assertIn("logic_alms_delta", payload["invalid_reason"])

    def test_register_budget_excess_fails(self) -> None:
        summaries = (
            (
                "Logic utilization (in ALMs) : 7,000\nTotal registers : 20,000\n"
                "Total block memory bits : 1,000,000\nTotal DSP Blocks : 2\nTotal PLLs : 3 / 6\n"
            ),
            (
                "Logic utilization (in ALMs) : 7,800\nTotal registers : 20,500\n"
                "Total block memory bits : 1,000,000\nTotal DSP Blocks : 2\nTotal PLLs : 3 / 6\n"
            ),
            (
                "Logic utilization (in ALMs) : 7,800\nTotal registers : 20,597\n"
                "Total block memory bits : 1,000,000\nTotal DSP Blocks : 2\nTotal PLLs : 3 / 6\n"
            ),
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
        baseline = BASE.replace("Info (332146): Worst-case setup slack is 0.500\n", "")
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
        patched = (
            (BASE + CUSTOM_SYNC)
            .replace("Total logic elements: 10,000", "Total logic elements: 10,801")
            .replace("Worst-case hold slack is 0.249", "Worst-case hold slack is 0.098")
        )
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
        patched = (
            BASE.replace(
                "Could Not be Calculated: 0.800",
                "Could Not be Calculated: 0.790",
            )
            + SYNC_ASSIGNMENTS
        )
        result, payload = self.run_check(BASE, patched)
        self.assertEqual(result.returncode, 1)
        self.assertIn("custom_synchronizer_mtbf_missing", payload["invalid_reason"])

    def test_missing_diagnostic_report_fails(self) -> None:
        result, payload = self.run_check(
            BASE, BASE + CUSTOM_SYNC, diagnostic_reports={}
        )
        self.assertEqual(result.returncode, 1)
        self.assertIn("diagnostic_cdc_report_missing", payload["invalid_reason"])

    def test_incomplete_or_negative_cdc_analysis_fails(self) -> None:
        reports = dict(VALID_DIAGNOSTIC_REPORTS)
        reports["menu.magik-diagnostic-cdc-net-delay.rpt"] = (
            "; set_net_delay ; 1.500 ; 8.000 ; 6.500 ; from ; to ;\n"
        )
        result, payload = self.run_check(
            BASE, BASE + CUSTOM_SYNC, diagnostic_reports=reports
        )
        self.assertEqual(result.returncode, 1)
        self.assertIn("diagnostic_cdc_analysis_count", payload["invalid_reason"])

    def test_duplicate_forward_net_delay_path_fails(self) -> None:
        reports = dict(VALID_DIAGNOSTIC_REPORTS)
        reports["menu.magik-diagnostic-cdc-net-delay.rpt"] = reports[
            "menu.magik-diagnostic-cdc-net-delay.rpt"
        ].replace(
            "ascal:ascal|o_readdataack_sync2~DUPLICATE ; ascal:ascal|avl_completion_ack_meta",
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

    def test_duplicated_raw_scaler_generation_path_fails(self) -> None:
        reports = dict(EXPERIMENTAL_DIAGNOSTIC_REPORTS)
        reports["menu.magik-diagnostic-cdc-net-delay.rpt"] = reports[
            "menu.magik-diagnostic-cdc-net-delay.rpt"
        ].replace("|source_generation ;", "|source_generation~DUPLICATE ;", 1)
        reports["menu.magik-diagnostic-metastability.rpt"] = reports[
            "menu.magik-diagnostic-metastability.rpt"
        ].replace("|source_generation ;", "|source_generation~DUPLICATE ;", 1)
        result, payload = self.run_check(
            BASE,
            BASE + EXPERIMENTAL_CUSTOM_SYNC,
            diagnostic_reports=reports,
            experimental_diagnostic=True,
        )
        self.assertEqual(result.returncode, 1)
        self.assertIn(
            "diagnostic_cdc_path_identity_mismatch", payload["invalid_reason"]
        )
        self.assertIn(
            "diagnostic_metastability_chain_missing", payload["invalid_reason"]
        )

    def test_duplicated_scaler_fetch_record_ready_path_fails(self) -> None:
        reports = dict(SCALER_FETCH_DIAGNOSTIC_REPORTS)
        reports["menu.magik-diagnostic-cdc-net-delay.rpt"] = reports[
            "menu.magik-diagnostic-cdc-net-delay.rpt"
        ].replace(
            "|record_ready ;",
            "|record_ready~DUPLICATE ;",
            1,
        )
        reports["menu.magik-diagnostic-metastability.rpt"] = reports[
            "menu.magik-diagnostic-metastability.rpt"
        ].replace(
            "|record_ready ;",
            "|record_ready~DUPLICATE ;",
            1,
        )
        result, payload = self.run_check(
            BASE,
            BASE + SCALER_FETCH_CUSTOM_SYNC,
            diagnostic_reports=reports,
            experimental_scaler_fetch=True,
        )
        self.assertEqual(result.returncode, 1)
        self.assertIn(
            "diagnostic_cdc_path_identity_mismatch", payload["invalid_reason"]
        )
        self.assertIn(
            "diagnostic_metastability_chain_missing", payload["invalid_reason"]
        )

    def test_missing_scheduler_snapshot_payload_path_fails(self) -> None:
        reports = dict(SCALER_FETCH_DIAGNOSTIC_REPORTS)
        missing = net_delay_detail(
            "mister_magik_scaler_scheduler_snapshot:scheduler_snapshot|compact_evidence[5]",
            "mister_magik_scaler_fetch_liveness_state:magik_scaler_fetch_liveness_state|scheduler_snapshot_capture[5]",
        )
        reports["menu.magik-diagnostic-cdc-net-delay.rpt"] = reports[
            "menu.magik-diagnostic-cdc-net-delay.rpt"
        ].replace(missing, "", 1)
        result, payload = self.run_check(
            BASE,
            BASE + SCALER_FETCH_CUSTOM_SYNC,
            diagnostic_reports=reports,
            experimental_scaler_fetch=True,
        )
        self.assertEqual(result.returncode, 1)
        self.assertIn("diagnostic_cdc_analysis_count", payload["invalid_reason"])

    def test_constant_scheduler_snapshot_marker_is_not_a_cdc_path(self) -> None:
        reports = dict(SCALER_FETCH_DIAGNOSTIC_REPORTS)
        reports["menu.magik-diagnostic-cdc-net-delay.rpt"] += net_delay_detail(
            "mister_magik_scaler_scheduler_snapshot:scheduler_snapshot|evidence_hold[0]",
            "mister_magik_scaler_fetch_liveness_state:magik_scaler_fetch_liveness_state|scheduler_snapshot_capture[0]",
        )
        result, payload = self.run_check(
            BASE,
            BASE + SCALER_FETCH_CUSTOM_SYNC,
            diagnostic_reports=reports,
            experimental_scaler_fetch=True,
        )
        self.assertEqual(result.returncode, 1)
        self.assertIn("diagnostic_cdc_analysis_count", payload["invalid_reason"])

    def test_pll_count_drift_fails(self) -> None:
        patched = (BASE + CUSTOM_SYNC).replace("Total PLLs: 3 / 6", "Total PLLs: 4 / 6")
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
        ].replace(
            "; Worst-Case MTBF (years) ; Greater than 10 Billion ;",
            "; Worst-Case MTBF (years) ; 1e+08 ;",
            1,
        )
        result, payload = self.run_check(
            BASE, BASE + CUSTOM_SYNC, diagnostic_reports=reports
        )
        self.assertEqual(result.returncode, 1)
        self.assertIn(
            "diagnostic_metastability_mtbf_below_minimum",
            payload["invalid_reason"],
        )

    def test_reverse_chain_requires_both_synchronization_registers(self) -> None:
        reports = dict(VALID_DIAGNOSTIC_REPORTS)
        reports["menu.magik-diagnostic-metastability.rpt"] = reports[
            "menu.magik-diagnostic-metastability.rpt"
        ].replace("; ascal:ascal|avl_completion_ack_sync ; ;\n", "", 1)
        result, payload = self.run_check(
            BASE, BASE + CUSTOM_SYNC, diagnostic_reports=reports
        )
        self.assertEqual(result.returncode, 1)
        self.assertIn(
            "diagnostic_metastability_chain_missing", payload["invalid_reason"]
        )

    def test_combined_mtbf_below_device_hour_gate_fails(self) -> None:
        reports = dict(VALID_DIAGNOSTIC_REPORTS)
        reports["menu.magik-diagnostic-metastability.rpt"] = reports[
            "menu.magik-diagnostic-metastability.rpt"
        ].replace(
            "; Worst-Case MTBF (years) ; Greater than 10 Billion ;",
            "; Worst-Case MTBF (years) ; 2e+08 ;",
        )
        result, payload = self.run_check(
            BASE, BASE + CUSTOM_SYNC, diagnostic_reports=reports
        )
        self.assertEqual(result.returncode, 1)
        self.assertNotIn(
            "diagnostic_metastability_mtbf_below_minimum", payload["invalid_reason"]
        )
        self.assertIn(
            "diagnostic_metastability_combined_mtbf_below_minimum",
            payload["invalid_reason"],
        )

    def test_similarly_named_chain_is_not_completion_evidence(self) -> None:
        reports = dict(VALID_DIAGNOSTIC_REPORTS)
        reports["menu.magik-diagnostic-metastability.rpt"] = reports[
            "menu.magik-diagnostic-metastability.rpt"
        ].replace(
            "; Source Node ; ascal:ascal|o_readdataack_sync2 ;",
            "; Source Node ; ascal:ascal|unrelated_readdataack_sync2 ;",
            1,
        )
        result, payload = self.run_check(
            BASE, BASE + CUSTOM_SYNC, diagnostic_reports=reports
        )
        self.assertEqual(result.returncode, 1)
        self.assertIn(
            "diagnostic_metastability_chain_missing", payload["invalid_reason"]
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
        self.assertEqual(sdc.count("set_net_delay -max 10.0"), 6)
        self.assertNotIn("set_max_skew", sdc)
        self.assertNotIn("set_false_path", sdc)

    def test_timing_report_retains_every_diagnostic_path(self) -> None:
        timing_report = (
            SCRIPT.parents[2]
            / "mister/platform/fpga/menu-vblank-latch/report_top_timing.tcl"
        ).read_text(encoding="utf-8")
        self.assertEqual(timing_report.count("-nworst 100"), 1)
        self.assertNotIn("-nworst 50", timing_report)


if __name__ == "__main__":
    unittest.main()
