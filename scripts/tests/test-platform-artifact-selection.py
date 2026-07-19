#!/usr/bin/env python3
# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

from __future__ import annotations

import importlib.util
import json
import tempfile
import unittest
from pathlib import Path

SCRIPT = Path(__file__).resolve().parents[1] / "release/platform/platform-artifact-selection.py"
SPEC = importlib.util.spec_from_file_location("platform_artifact_selection", SCRIPT)
assert SPEC and SPEC.loader
selection = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(selection)


class ArtifactSelectionTests(unittest.TestCase):
    def setUp(self) -> None:
        self.temp = tempfile.TemporaryDirectory(prefix="platform-artifact-selection-")
        self.root = Path(self.temp.name)

    def tearDown(self) -> None:
        self.temp.cleanup()

    def write(self, name: str, payload: object) -> Path:
        path = self.root / name
        path.write_text(json.dumps(payload))
        return path

    def artifact(self, artifact_id: int, *, name: str = "wanted", created: str = "2026-01-01", expired: bool = False, branch: str = "main", fork: bool = False) -> dict[str, object]:
        return {
            "id": artifact_id,
            "name": name,
            "expired": expired,
            "created_at": created,
            "workflow_run": {
                "id": artifact_id + 100,
                "head_branch": branch,
                "head_sha": f"{artifact_id:040x}",
                "repository_id": 1,
                "head_repository_id": 2 if fork else 1,
            },
        }

    def test_candidates_are_newest_first_and_exact(self) -> None:
        path = self.write("artifacts.json", {"artifacts": [
            self.artifact(1, created="2026-01-01"),
            self.artifact(2, created="2026-01-03"),
            self.artifact(3, name="other", created="2026-01-04"),
        ]})
        self.assertEqual([item["id"] for item in selection.candidates(path, "wanted")], [2, 1])

    def test_expired_wrong_branch_and_fork_artifacts_are_rejected(self) -> None:
        path = self.write("artifacts.json", {"artifacts": [
            self.artifact(1, expired=True), self.artifact(2, branch="feature"), self.artifact(3, fork=True),
        ]})
        self.assertEqual(selection.candidates(path, "wanted"), [])

    def test_paginated_response_is_supported(self) -> None:
        path = self.write("pages.json", [{"artifacts": [self.artifact(1)]}, {"artifacts": [self.artifact(2)]}])
        self.assertEqual(len(selection.candidates(path, "wanted")), 2)

    def test_malformed_artifact_response_is_rejected(self) -> None:
        with self.assertRaisesRegex(ValueError, "invalid Actions artifacts"):
            selection.candidates(self.write("malformed.json", {"wrong": []}), "wanted")

    def test_only_completed_unified_manual_runs_are_eligible(self) -> None:
        base = {
            "status": "completed", "conclusion": "failure",
            "workflowName": selection.WORKFLOW, "headBranch": "main",
            "event": "workflow_dispatch", "headSha": "a" * 40,
        }
        path = self.write("run.json", base)
        self.assertTrue(selection.eligible_run(path, "a" * 40))
        for key, value in (("status", "in_progress"), ("workflowName", "Other"), ("headBranch", "feature"), ("event", "push"), ("headSha", "b" * 40)):
            altered = dict(base)
            altered[key] = value
            self.assertFalse(selection.eligible_run(self.write(f"bad-{key}.json", altered), "a" * 40))


if __name__ == "__main__":
    unittest.main()
