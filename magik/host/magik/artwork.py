"""Deliver the app's card pack to the fixed Dev artwork slot before startup."""

from __future__ import annotations

import hashlib
import json
import re
import struct
from pathlib import Path

from .client import AgentError
from .protocol import MAX_BODY_BYTES
from .results import append_event

SOURCE_BYTES = 360 * 504 * 3
MAX_INDEX_BYTES = 128 * 1024
MAX_FILES = 120
ROOT_KEYS = {
    f"root:{name}"
    for name in (
        "arcade",
        "consoles",
        "computers",
        "handhelds",
        "favourites",
        "settings",
    )
}


def bundle(root: Path) -> tuple[bytes, int]:
    raw = (root / "index.json").read_bytes()
    if not raw or len(raw) > MAX_INDEX_BYTES:
        raise ValueError("oversized card artwork index")
    index = json.loads(raw)
    if (
        index.get("schema"),
        index.get("width"),
        index.get("height"),
        index.get("format"),
    ) != (1, 360, 504, "RGB888"):
        raise ValueError("unsupported card artwork index")
    cards = index.get("cards")
    if not isinstance(cards, dict) or not ROOT_KEYS <= cards.keys():
        raise ValueError("card artwork index is missing root cards")
    files = {}
    for source in cards.values():
        name, digest = source["file"], source["sha256"]
        if (
            not re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9_.-]*\.rgb888", name)
            or not re.fullmatch(r"[0-9a-f]{64}", digest)
            or not isinstance(source.get("contains_name", False), bool)
        ):
            raise ValueError("invalid card artwork source")
        if name in files and files[name] != digest:
            raise ValueError("conflicting card artwork checksums")
        files[name] = digest
    if not files or len(files) > MAX_FILES:
        raise ValueError("card artwork file count exceeds limit")
    payload = bytearray(struct.pack(">I", len(raw)))
    payload.extend(raw)
    for name in sorted(files):
        path = root / name
        if (
            path.is_symlink()
            or not path.is_file()
            or path.stat().st_size != SOURCE_BYTES
        ):
            raise ValueError(f"missing or incomplete card artwork: {name}")
        pixels = path.read_bytes()
        if (
            len(pixels) != SOURCE_BYTES
            or hashlib.sha256(pixels).hexdigest() != files[name]
        ):
            raise ValueError(f"card artwork checksum mismatch: {name}")
        payload.extend(pixels)
    if len(payload) > MAX_BODY_BYTES:
        raise ValueError("card artwork bundle exceeds native body limit")
    return bytes(payload), len(files)


def _state(agent) -> dict:
    response, _ = agent._request("card-artwork-state", attempts=1, timeout=20)
    if response.operation == "error":
        raise AgentError.from_fields(response.fields)
    if response.operation != "card-artwork-ready":
        raise AgentError("unexpected artwork state response")
    return dict(response.fields)


def ensure(root: Path, agent, run: Path) -> bool:
    """Return whether artwork changed, so a warm launcher is restarted to see it."""
    payload, count = bundle(root)
    digest = hashlib.sha256(payload).hexdigest()
    if _state(agent).get("sha256") == digest:
        append_event(
            run,
            {
                "phase": "artwork",
                "outcome": "current",
                "files": count,
                "bytes": 0,
                "sha256": digest,
            },
        )
        print(f"Artwork: {count} card images already installed")
        return False
    append_event(
        run,
        {
            "phase": "artwork",
            "outcome": "uploading",
            "files": count,
            "bytes": len(payload),
            "sha256": digest,
        },
    )
    try:
        response, _ = agent._request(
            "card-artwork-install", {"sha256": digest}, payload, attempts=1, timeout=180
        )
    except OSError:
        # One read-only reconciliation; never repeat an ambiguous installation.
        if _state(agent).get("sha256") != digest:
            raise
        append_event(
            run, {"phase": "artwork", "outcome": "reconciled", "sha256": digest}
        )
    else:
        if response.operation == "error":
            raise AgentError.from_fields(response.fields)
        if (
            response.operation != "card-artwork-ready"
            or response.fields.get("sha256") != digest
            or response.fields.get("files") != count
        ):
            raise AgentError("installed artwork receipt does not match")
        append_event(
            run, {"phase": "artwork", "outcome": "installed", "sha256": digest}
        )
    print(f"Artwork: installed {count} card images in the Dev artwork folder")
    return True
