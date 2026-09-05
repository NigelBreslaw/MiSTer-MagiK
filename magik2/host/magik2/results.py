"""Small per-command result bundles; never a workflow database."""

from __future__ import annotations

import json
import os
import subprocess
import time
import uuid
from collections.abc import Mapping
from pathlib import Path
from typing import Any


def source_context(device: str) -> dict[str, object]:
    """Capture Git provenance without letting unrelated changes affect builds."""
    try:
        revision = subprocess.run(
            ["git", "rev-parse", "HEAD"], check=True, capture_output=True, text=True
        ).stdout.strip()
        dirty = bool(
            subprocess.run(
                ["git", "status", "--porcelain"],
                check=True,
                capture_output=True,
                text=True,
            ).stdout
        )
    except (OSError, subprocess.CalledProcessError):
        revision, dirty = "unknown", None
    return {"mister_ip": device, "git_revision": revision, "git_dirty": dirty}


def create_run(root: Path, operation: str, source: Mapping[str, Any]) -> Path:
    directory = (
        root
        / f"{time.strftime('%Y%m%dT%H%M%SZ', time.gmtime())}-{uuid.uuid4().hex[:12]}"
    )
    directory.mkdir(parents=True, exist_ok=False)
    payload = {
        "operation": operation,
        "source": dict(source),
        "events": "events.jsonl",
        "logs": "logs.txt",
    }
    (directory / "logs.txt").write_text("")
    _atomic_json(directory / "run.json", payload)
    return directory


def append_event(directory: Path, event: Mapping[str, Any]) -> None:
    with (directory / "events.jsonl").open("a", encoding="utf-8") as output:
        output.write(
            json.dumps(
                {"time_ns": time.monotonic_ns(), **dict(event)},
                sort_keys=True,
                separators=(",", ":"),
            )
            + "\n"
        )


def _atomic_json(destination: Path, value: Mapping[str, Any]) -> None:
    temporary = destination.with_suffix(".tmp")
    temporary.write_text(
        json.dumps(dict(value), indent=2, sort_keys=True) + "\n", encoding="utf-8"
    )
    os.replace(temporary, destination)


def retain_diagnostics(directory: Path, agent) -> None:
    try:
        evidence = agent.diagnostics()
        _atomic_json(directory / "device.json", evidence)
        with (directory / "logs.txt").open("a") as output:
            output.write(
                str(evidence.get("probe_log", ""))[-16384:]
                + "\n"
                + str(evidence.get("agent_log", ""))[-16384:]
                + "\n"
            )
        append_event(directory, {"phase": "diagnostics", "outcome": "retained"})
    except Exception as error:
        append_event(
            directory,
            {"phase": "diagnostics", "outcome": "unavailable", "error": str(error)},
        )


def finalize(directory: Path, code: int, elapsed_ms: int) -> None:
    payload = json.loads((directory / "run.json").read_text())
    events = [
        json.loads(line)
        for line in (directory / "events.jsonl").read_text().splitlines()
    ]
    payload.update(
        {
            "outcome": "passed" if code == 0 else "failed",
            "exit_code": code,
            "elapsed_ms": elapsed_ms,
        }
    )
    payload["artifact"] = next(
        (e for e in reversed(events) if e.get("phase") == "artifact"), None
    )
    payload["agent"] = next(
        (e for e in reversed(events) if e.get("phase") == "agent"), None
    )
    payload["failures"] = [
        e for e in events if e.get("outcome") in {"failed", "unavailable"}
    ]
    _atomic_json(directory / "run.json", payload)
