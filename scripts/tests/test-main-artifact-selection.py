#!/usr/bin/env python3
# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

import importlib.util
import unittest
from pathlib import Path

SCRIPT = Path(__file__).resolve().parents[1] / "release/platform/main-artifact-selection.py"
SPEC = importlib.util.spec_from_file_location("main_artifact_selection", SCRIPT)
assert SPEC and SPEC.loader
selection = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(selection)


class MainArtifactSelectionTests(unittest.TestCase):
    name = "mister-magik-main-v0.1-exact"

    def artifact(self, run_id: int, *, expired: bool = False, created: str = "2026-07-19T00:00:00Z") -> dict:
        return {"name": self.name, "expired": expired, "created_at": created, "workflow_run": {"id": run_id}}

    def test_hit_prefers_latest_nonexpired_exact_artifact(self) -> None:
        payload = {"artifacts": [
            self.artifact(1, created="2026-07-18T00:00:00Z"),
            self.artifact(2, created="2026-07-19T00:00:00Z"),
            {**self.artifact(3), "name": "wrong-component"},
        ]}
        self.assertEqual(selection.candidate_run_ids(payload, self.name), [2, 1])

    def test_expired_exact_artifact_is_a_cache_miss(self) -> None:
        self.assertEqual(selection.candidate_run_ids({"artifacts": [self.artifact(1, expired=True)]}, self.name), [])

    def test_malformed_artifact_origin_is_rejected(self) -> None:
        artifact = self.artifact(1)
        artifact["workflow_run"] = {}
        with self.assertRaisesRegex(ValueError, "origin"):
            selection.candidate_run_ids({"artifacts": [artifact]}, self.name)

    def test_only_successful_completed_main_branch_run_is_eligible(self) -> None:
        valid = {"status": "completed", "conclusion": "success", "workflowName": "Main_MiSTer component", "headBranch": "main"}
        self.assertTrue(selection.eligible_run(valid))
        for key, value in (("status", "in_progress"), ("conclusion", "failure"), ("workflowName", "Other"), ("headBranch", "feature")):
            invalid = dict(valid)
            invalid[key] = value
            self.assertFalse(selection.eligible_run(invalid), key)

    def test_successful_platform_promotion_run_is_eligible(self) -> None:
        self.assertTrue(selection.eligible_run({
            "status": "completed",
            "conclusion": "success",
            "workflowName": "Promote MiSTer MagiK Platform Bundle",
            "headBranch": "main",
        }))


if __name__ == "__main__":
    unittest.main()
