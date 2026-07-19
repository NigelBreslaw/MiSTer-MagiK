#!/usr/bin/env python3
# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

"""Select reusable Main component artifacts from GitHub Actions metadata."""

from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path

WORKFLOWS = {"Main_MiSTer component", "Promote MiSTer MagiK Platform Bundle"}


def candidate_run_ids(payload: object, artifact_name: str) -> list[int]:
    if not isinstance(payload, dict) or not isinstance(payload.get("artifacts"), list):
        raise ValueError("malformed artifacts response")
    candidates: list[tuple[str, int]] = []
    for artifact in payload["artifacts"]:
        if not isinstance(artifact, dict) or artifact.get("name") != artifact_name or artifact.get("expired") is not False:
            continue
        workflow_run = artifact.get("workflow_run")
        run_id = workflow_run.get("id") if isinstance(workflow_run, dict) else None
        created_at = artifact.get("created_at")
        if isinstance(run_id, bool) or not isinstance(run_id, int) or run_id < 1 or not isinstance(created_at, str):
            raise ValueError("malformed artifact origin")
        candidates.append((created_at, run_id))
    return list(dict.fromkeys(run_id for _, run_id in sorted(candidates, reverse=True)))


def eligible_run(payload: object) -> bool:
    return (
        isinstance(payload, dict)
        and payload.get("status") == "completed"
        and payload.get("conclusion") == "success"
        and payload.get("workflowName") in WORKFLOWS
        and payload.get("headBranch") == "main"
        and set(payload) == {"status", "conclusion", "workflowName", "headBranch"}
    )


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    candidates = commands.add_parser("candidates")
    candidates.add_argument("--artifacts", required=True, type=Path)
    candidates.add_argument("--name", required=True)
    eligible = commands.add_parser("eligible")
    eligible.add_argument("--run", required=True, type=Path)
    args = parser.parse_args()
    try:
        if args.command == "candidates":
            payload = json.loads(args.artifacts.read_text())
            for run_id in candidate_run_ids(payload, args.name):
                print(run_id)
        else:
            return 0 if eligible_run(json.loads(args.run.read_text())) else 1
    except (OSError, json.JSONDecodeError, ValueError) as error:
        print(f"Main artifact selection failed: {error}", file=sys.stderr)
        return 2
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
