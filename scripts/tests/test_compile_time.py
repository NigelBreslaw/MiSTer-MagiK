"""The timing utility compares exactly two compatible successful samples."""

import copy
import unittest
from scripts.magik_ci.compile_time import compare


class CompileComparisonTests(unittest.TestCase):
    def test_two_sample_comparison_and_invalid_evidence(self):
        baseline = dict(
            schema=1,
            target="app",
            kind="incremental",
            machine="mac",
            architecture="arm64",
            recipe=["dev"],
            samples=[
                dict(repetition=1, seconds=2, exit_code=0),
                dict(repetition=2, seconds=4, exit_code=0),
            ],
        )
        candidate = copy.deepcopy(baseline)
        candidate["samples"][0]["seconds"] = 1
        candidate["samples"][1]["seconds"] = 2
        self.assertEqual(compare(baseline, candidate)["change_percent"], -50)
        for field, value in (
            ("samples", baseline["samples"][:1]),
            ("kind", "cold"),
            ("error", "failed"),
        ):
            invalid = dict(candidate, **{field: value})
            with self.assertRaises(ValueError):
                compare(baseline, invalid)
        for value in (0, -1, float("nan"), float("inf"), True):
            invalid = copy.deepcopy(candidate)
            invalid["samples"][0]["seconds"] = value
            with self.assertRaises(ValueError):
                compare(baseline, invalid)
