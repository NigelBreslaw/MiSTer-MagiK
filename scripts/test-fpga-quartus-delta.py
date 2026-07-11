#!/usr/bin/env python3
"""Fixture tests for check-fpga-quartus-delta.py."""

from __future__ import annotations

import json
from pathlib import Path
import subprocess
import tempfile
import unittest


SCRIPT = Path(__file__).with_name("check-fpga-quartus-delta.py")

BASE = """\
Warning (10001): inherited warning File: /work/sys/top.v Line: 7
Info (332146): Worst-case setup slack is 0.132
    Info (332119):     Slack       End Point TNS Clock
    Info (332119): ========= =================== =====================
    Info (332119):     0.132               0.000 clk_sys
Info (332146): Worst-case hold slack is 0.249
    Info (332119):     Slack       End Point TNS Clock
    Info (332119): ========= =================== =====================
    Info (332119):     0.249               0.000 clk_sys
Info (332114): Report Metastability: Found 5 synchronizer chains.
Info (332114): Fraction of Chains for which MTBFs Could Not be Calculated: 0.800
; Unconstrained Output Port Paths ; 10 ; 10 ;
Info (332102): Design is not fully constrained for setup requirements
Info (332102): Design is not fully constrained for hold requirements
"""

CUSTOM_SYNC = """\
; SYNCHRONIZER_IDENTIFICATION ; FORCED_IF_ASYNCHRONOUS ; - ; vbl_meta ;
; SYNCHRONIZER_IDENTIFICATION ; FORCED_IF_ASYNCHRONOUS ; - ; vbl_sys ;
Info (332114): Report Metastability: Found 6 synchronizer chains.
Info (332114): Fraction of Chains for which MTBFs Could Not be Calculated: 0.700
"""


class QuartusDeltaTest(unittest.TestCase):
    def run_check(self, stock: str, patched: str) -> tuple[subprocess.CompletedProcess[str], dict[str, object]]:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            stock_path = root / "stock.log"
            patched_path = root / "patched.log"
            stock_path.write_text(stock, encoding="utf-8")
            patched_path.write_text(patched, encoding="utf-8")
            result = subprocess.run(
                [str(SCRIPT), "--stock", str(stock_path), "--patched", str(patched_path), "--json"],
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

    def test_new_warning_fails_even_when_warning_code_is_inherited(self) -> None:
        result, payload = self.run_check(BASE, BASE + "Warning (10001): different warning\n" + CUSTOM_SYNC)
        self.assertEqual(result.returncode, 1)
        self.assertIn("warning_added", payload["invalid_reason"])

    def test_missing_inherited_warning_fails_exact_baseline(self) -> None:
        result, payload = self.run_check(BASE, BASE.replace("Warning (10001): inherited warning File: /work/sys/top.v Line: 7\n", "") + CUSTOM_SYNC)
        self.assertEqual(result.returncode, 1)
        self.assertIn("warning_baseline_mismatch", payload["invalid_reason"])

    def test_negative_slack_and_nonzero_tns_fail(self) -> None:
        patched = (BASE + CUSTOM_SYNC).replace("setup slack is 0.132", "setup slack is -0.001").replace("0.132               0.000", "-0.001              -0.010")
        result, payload = self.run_check(BASE, patched)
        self.assertEqual(result.returncode, 1)
        self.assertIn("setup_slack_negative", payload["invalid_reason"])
        self.assertIn("tns_nonzero", payload["invalid_reason"])

    def test_new_unconstrained_evidence_fails(self) -> None:
        result, payload = self.run_check(BASE, BASE + "Info: 1 unconstrained output path: magik_route\n" + CUSTOM_SYNC)
        self.assertEqual(result.returncode, 1)
        self.assertIn("unconstrained_added", payload["invalid_reason"])

    def test_global_mtbf_is_not_custom_chain_evidence(self) -> None:
        patched = BASE + "Info (332114): Worst-Case MTBF of Design is 1e+09 years\n"
        result, payload = self.run_check(BASE, patched)
        self.assertEqual(result.returncode, 1)
        self.assertIn("custom_synchronizer_missing", payload["invalid_reason"])

    def test_named_chain_without_mtbf_fails(self) -> None:
        assignments = "; SYNCHRONIZER_IDENTIFICATION ; FORCED_IF_ASYNCHRONOUS ; - ; vbl_meta ;\n; SYNCHRONIZER_IDENTIFICATION ; FORCED_IF_ASYNCHRONOUS ; - ; vbl_sys ;\n"
        result, payload = self.run_check(BASE, BASE + assignments)
        self.assertEqual(result.returncode, 1)
        self.assertIn("custom_synchronizer_mtbf_missing", payload["invalid_reason"])


if __name__ == "__main__":
    unittest.main()
