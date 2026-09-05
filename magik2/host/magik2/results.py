"""Small per-command result bundles; never a workflow database."""

from __future__ import annotations

import json
import os
import time
import uuid
from collections.abc import Mapping
from pathlib import Path
from typing import Any


def create_run(root: Path, operation: str, source: Mapping[str, Any]) -> Path:
    directory = root / f"{time.strftime('%Y%m%dT%H%M%SZ', time.gmtime())}-{uuid.uuid4().hex[:12]}"
    directory.mkdir(parents=True, exist_ok=False)
    payload = {"operation": operation, "source": dict(source), "events": "events.jsonl", "logs": "logs.txt"}
    _atomic_json(directory / "run.json", payload)
    return directory


def append_event(directory: Path, event: Mapping[str, Any]) -> None:
    with (directory / "events.jsonl").open("a", encoding="utf-8") as output:
        output.write(json.dumps(dict(event), sort_keys=True, separators=(",", ":")) + "\n")


def _atomic_json(destination: Path, value: Mapping[str, Any]) -> None:
    temporary = destination.with_suffix(".tmp")
    temporary.write_text(json.dumps(dict(value), indent=2, sort_keys=True) + "\n", encoding="utf-8")
    os.replace(temporary, destination)
