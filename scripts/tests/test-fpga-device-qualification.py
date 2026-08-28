#!/usr/bin/env python3
# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

from __future__ import annotations

import subprocess
import tempfile
import unittest
from pathlib import Path

SCRIPT = (
    Path(__file__).resolve().parents[1] / "checks/check-fpga-device-qualification.py"
)


class QualificationTest(unittest.TestCase):
    def row(self, path: Path, work: int, *, drop: int = 0) -> None:
        path.write_text(
            "max_scroll_gate_tsv\tvalid=1 work_p99="
            + str(work)
            + " latch_deadline_misses=0 visual_latch_misses=0 buffer_alternation_failures=0"
            + " flip_counter_gaps=0 fpga_drop_count_max="
            + str(drop)
            + " fpga_counters_advanced=1\n"
        )

    def run_case(
        self, candidate_work: int, *, drop: int = 0
    ) -> subprocess.CompletedProcess[str]:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            paths = [
                root / f"{name}{index}"
                for name in ("bh", "ba", "ch", "ca")
                for index in (1, 2)
            ]
            for path in paths[:4]:
                self.row(path, 1000)
            for path in paths[4:]:
                self.row(path, candidate_work, drop=drop)
            command = [str(SCRIPT)]
            for option, selected in (
                ("--baseline-home", paths[0:2]),
                ("--baseline-arcade", paths[2:4]),
                ("--candidate-home", paths[4:6]),
                ("--candidate-arcade", paths[6:8]),
            ):
                for path in selected:
                    command.extend((option, str(path)))
            return subprocess.run(command, text=True, capture_output=True, check=False)

    def test_matching_samples_pass(self) -> None:
        self.assertEqual(self.run_case(1029).returncode, 0)

    def test_regression_or_drop_fails(self) -> None:
        self.assertNotEqual(self.run_case(1031).returncode, 0)
        self.assertNotEqual(self.run_case(1000, drop=1).returncode, 0)


if __name__ == "__main__":
    unittest.main()
