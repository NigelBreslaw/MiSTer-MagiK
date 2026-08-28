#!/usr/bin/env python3
# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

import sys
import tempfile
import unittest
from pathlib import Path

REPORTS = Path(__file__).resolve().parents[1] / "bench" / "reports"
sys.path.insert(0, str(REPORTS))

from frame_profile_schema import int_field, phase_stats, read_rows


class FrameProfileSchemaTests(unittest.TestCase):
    def test_legacy_alias_numeric_coercion_and_stats_are_shared(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            fixture = Path(directory) / "frames.tsv"
            fixture.write_text(
                "frame\twall_us\toverlay_present_us\n0\t10.9\t4\n1\tbad\t8.8\n",
                encoding="utf-8",
            )
            rows = read_rows(fixture)
        self.assertEqual(
            [int_field(row, "arcade_list_present_us") for row in rows], [4, 8]
        )
        self.assertEqual(
            phase_stats(rows, "wall_us"),
            {"avg": 5, "p50": 0, "p95": 10, "p99": 10, "max": 10},
        )

    def test_duplicate_headers_and_extra_columns_are_rejected(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            fixture = Path(directory) / "bad.tsv"
            fixture.write_text("frame\tframe\n0\t1\n", encoding="utf-8")
            with self.assertRaisesRegex(ValueError, "duplicate columns"):
                read_rows(fixture)
            fixture.write_text("frame\twall_us\n0\t1\textra\n", encoding="utf-8")
            with self.assertRaisesRegex(ValueError, "extra columns"):
                read_rows(fixture)


if __name__ == "__main__":
    unittest.main()
