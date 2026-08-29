# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

from __future__ import annotations

import json
import subprocess
from pathlib import Path

from .common import github_output


def platform_candidates(artifacts: Path, name: str) -> list[dict[str, object]]:
    payload = json.loads(artifacts.read_text(encoding="utf-8"))
    values = payload.get("artifacts", payload) if isinstance(payload, dict) else payload
    if isinstance(values, list):
        flattened: list[object] = []
        for page in values:
            if isinstance(page, dict) and isinstance(page.get("artifacts"), list):
                flattened.extend(page["artifacts"])
            elif isinstance(page, list):
                flattened.extend(page)
            else:
                flattened.append(page)
        values = flattened
    if not isinstance(values, list):
        return []
    return [
        item
        for item in values
        if isinstance(item, dict)
        and item.get("name") == name
        and not item.get("expired", False)
    ]


def platform_eligible_run(path: Path, head_sha: str) -> bool:
    payload = json.loads(path.read_text(encoding="utf-8"))
    origin = payload.get("workflow_run", payload)
    actual_sha = origin.get("head_sha", origin.get("headSha"))
    branch = origin.get("head_branch", origin.get("headBranch"))
    return bool(
        actual_sha == head_sha
        and branch in {"main", "mister-magik"}
        and origin.get("status", "completed") == "completed"
        and origin.get("conclusion", "success") == "success"
    )


def require_alpha_promotion(channel: str, alpha_sha: str, candidate_sha: str) -> None:
    if channel == "alpha" and alpha_sha != candidate_sha:
        raise ValueError("alpha promotion is required before stable publication")


def host_assurance(paths: list[str]) -> None:
    """Run the bounded host checks selected by a CI path group."""
    root = Path.cwd()
    checks: list[tuple[Path, list[str]]] = [
        (root / "scripts/checks/check-repository-layout.py", []),
        (root / "scripts/checks/check-unified-agent-surface.py", []),
    ]
    if any(path.startswith(("scripts", "docs")) for path in paths):
        checks.append(
            (
                root / "scripts/checks/check-font-text-contract.py",
                ["--repository", str(root), "--all"],
            )
        )
    if any("visual-baselines/launcher" in path for path in paths):
        checks.append((root / "scripts/tests/test-launcher-contract.py", []))
    for check, arguments in checks:
        subprocess.run([str(check), *arguments], cwd=root, check=True)
    if any("visual-baselines/launcher" in path for path in paths):
        subprocess.run(
            [
                "cargo",
                "run",
                "--manifest-path",
                "apps/mister/Cargo.toml",
                "--bin",
                "mister-magik-ui-preview",
                "--no-default-features",
                "--features",
                "ui-preview",
                "--",
                "--check-baselines",
                "apps/mister/tests/visual-baselines/launcher",
            ],
            cwd=root,
            check=True,
        )


def write_plan(path: Path | None, value: dict[str, object]) -> None:
    github_output(path, value)
    print(json.dumps(value, sort_keys=True))
