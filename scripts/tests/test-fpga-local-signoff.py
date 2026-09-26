#!/usr/bin/env python3
"""Offline safety tests for local synthesis orchestration; no containers run."""

import json
import sys
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[2]))
from scripts.magik_ci import fpga_local


class LocalSignoff(unittest.TestCase):
    def test_ref_mismatch_fails_before_any_container_call(self):
        with patch.object(
            fpga_local, "run", side_effect=["candidate", "other-main"]
        ) as run:
            with self.assertRaisesRegex(ValueError, "local main"):
                fpga_local.identity(Path("/fixture"))
            self.assertEqual(run.call_count, 2)

    def test_dirty_candidate_fails_before_any_container_call(self):
        with patch.object(
            fpga_local, "run", side_effect=["candidate", "candidate", " M source.sv"]
        ):
            with self.assertRaisesRegex(ValueError, "clean committed"):
                fpga_local.identity(Path("/fixture"))

    def test_missing_runtime_preserves_completed_evidence(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            old = root / "runs" / "old"
            old.mkdir(parents=True)
            report = old / "completed.json"
            report.write_text('{"saved":true}')
            with (
                patch.object(fpga_local, "identity", return_value={}),
                patch.object(fpga_local, "run") as run,
            ):
                with self.assertRaisesRegex(ValueError, "runtime missing"):
                    fpga_local.signoff(root, root)
                run.assert_not_called()
            self.assertEqual(json.loads(report.read_text()), {"saved": True})

    def test_relative_cache_override_is_rejected(self):
        with self.assertRaisesRegex(ValueError, "absolute"):
            fpga_local.local_root(Path("/fixture"), Path("relative"))

    def test_shared_default_cache_uses_primary_checkout(self):
        with (
            patch.object(fpga_local, "run", return_value="/primary/.git"),
            patch.dict("os.environ", {}, clear=True),
        ):
            self.assertEqual(
                fpga_local.local_root(Path("/other-worktree")),
                Path("/primary/build/fpga-local-apple"),
            )


if __name__ == "__main__":
    unittest.main()
