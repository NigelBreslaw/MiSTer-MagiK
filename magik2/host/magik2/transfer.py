"""One full, verified SD upload. Never rebuild or start the application."""

from __future__ import annotations
import hashlib
import json
import time
from pathlib import Path
from .client import AgentError
from .results import append_event, retain_diagnostics


def transfer_check(agent, artifact: Path, run: Path) -> int:
    payload = artifact.read_bytes()
    expected = hashlib.sha256(payload).hexdigest()
    if not payload:
        raise ValueError("comparison artifact is empty")
    started = time.monotonic()
    try:
        reply, _ = agent._request("transfer-check", {"sha256": expected}, payload)
        if reply.operation != "transfer-saved":
            raise AgentError.from_fields(reply.fields)
        result = dict(reply.fields)
        if result.get("sha256") != expected or result.get("bytes") != len(payload):
            raise AgentError("saved comparison artifact does not match")
        rate = result["bytes_per_second"]
        result.update(
            system="magik2",
            elapsed_ms=round((time.monotonic() - started) * 1000),
            mb_per_second=rate / 1_000_000,
            mbit_per_second=rate * 8 / 1_000_000,
        )
        append_event(run, {"phase": "transfer", "outcome": "passed", **result})
        print(json.dumps(result, sort_keys=True))
        return 0
    finally:
        retain_diagnostics(run, agent)
