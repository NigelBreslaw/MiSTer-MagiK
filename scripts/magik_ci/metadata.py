from __future__ import annotations

import json
from pathlib import Path

from .common import github_output


def platform_candidates(artifacts: Path, name: str) -> list[dict[str, object]]:
    payload = json.loads(artifacts.read_text(encoding="utf-8"))
    values = payload.get("artifacts", payload) if isinstance(payload, dict) else payload
    if (
        isinstance(values, list)
        and values
        and all(isinstance(page, list) for page in values)
    ):
        values = [item for page in values for item in page]
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
    return bool(
        origin.get("head_sha") == head_sha
        and origin.get("head_branch") in {"main", "mister-magik"}
        and origin.get("status", "completed") == "completed"
        and origin.get("conclusion", "success") == "success"
    )


def require_alpha_promotion(channel: str, alpha_sha: str, candidate_sha: str) -> None:
    if channel == "alpha" and alpha_sha != candidate_sha:
        raise ValueError("alpha promotion is required before stable publication")


def host_assurance(paths: list[str]) -> None:
    """Validate that requested host paths exist; detailed checks live in scripts/checks."""
    missing = [path for path in paths if not Path(path).exists()]
    if missing:
        raise FileNotFoundError(", ".join(missing))


def write_plan(path: Path | None, value: dict[str, object]) -> None:
    github_output(path, value)
    print(json.dumps(value, sort_keys=True))
