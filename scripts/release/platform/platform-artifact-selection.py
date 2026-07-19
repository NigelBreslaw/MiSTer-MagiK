#!/usr/bin/env python3
# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

"""Select reusable artifacts produced by the unified platform workflow."""

from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path

WORKFLOW = "Build MiSTer MagiK Platform"


class SelectionError(ValueError):
    pass


def load_artifacts(path: Path) -> list[dict[str, object]]:
    payload = json.loads(path.read_text())
    pages = payload if isinstance(payload, list) else [payload]
    artifacts: list[dict[str, object]] = []
    for page in pages:
        if not isinstance(page, dict) or not isinstance(page.get("artifacts"), list):
            raise SelectionError("invalid Actions artifacts response")
        artifacts.extend(item for item in page["artifacts"] if isinstance(item, dict))
    return artifacts


def candidates(path: Path, name: str) -> list[dict[str, object]]:
    result = []
    for artifact in load_artifacts(path):
        origin = artifact.get("workflow_run")
        if (
            artifact.get("name") != name
            or artifact.get("expired") is not False
            or not isinstance(origin, dict)
            or origin.get("head_branch") != "main"
            or origin.get("repository_id") != origin.get("head_repository_id")
            or not isinstance(artifact.get("id"), int)
            or not isinstance(origin.get("id"), int)
        ):
            continue
        result.append(artifact)
    return sorted(result, key=lambda item: str(item.get("created_at", "")), reverse=True)


def eligible_run(path: Path, expected_sha: str) -> bool:
    payload = json.loads(path.read_text())
    return (
        isinstance(payload, dict)
        and payload.get("status") == "completed"
        and payload.get("conclusion") in {"success", "failure", "cancelled"}
        and payload.get("workflowName") == WORKFLOW
        and payload.get("headBranch") == "main"
        and payload.get("event") == "workflow_dispatch"
        and payload.get("headSha") == expected_sha
    )


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    list_parser = commands.add_parser("candidates")
    list_parser.add_argument("--artifacts", required=True, type=Path)
    list_parser.add_argument("--name", required=True)
    run_parser = commands.add_parser("eligible-run")
    run_parser.add_argument("--run", required=True, type=Path)
    run_parser.add_argument("--head-sha", required=True)
    args = parser.parse_args()
    try:
        if args.command == "candidates":
            for artifact in candidates(args.artifacts, args.name):
                origin = artifact["workflow_run"]
                print(f"{artifact['id']}\t{origin['id']}\t{origin['head_sha']}")
        elif eligible_run(args.run, args.head_sha):
            return 0
        else:
            return 1
    except (OSError, json.JSONDecodeError, SelectionError) as error:
        print(f"platform artifact selection error: {error}", file=sys.stderr)
        return 2
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
