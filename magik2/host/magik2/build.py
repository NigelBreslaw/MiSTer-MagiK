"""Probe input fingerprints; unrelated checkout state is intentionally ignored."""

from __future__ import annotations

import hashlib
import json
import os
from collections.abc import Iterable
from pathlib import Path

INPUT_NAMES = {"Cargo.toml", "Cargo.lock", "build.rs"}
INPUT_SUFFIXES = {".rs", ".slint"}


def relevant_inputs(probe_root: Path) -> list[Path]:
    return sorted(
        path
        for path in probe_root.rglob("*")
        if path.is_file() and (path.name in INPUT_NAMES or path.suffix in INPUT_SUFFIXES)
    )


def source_fingerprint(probe_root: Path) -> str:
    digest = hashlib.sha256()
    for path in relevant_inputs(probe_root):
        digest.update(path.relative_to(probe_root).as_posix().encode() + b"\0")
        digest.update(path.read_bytes())
        digest.update(b"\0")
    return digest.hexdigest()


def needs_build(cache_file: Path, fingerprint: str) -> bool:
    try:
        cached = json.loads(cache_file.read_text(encoding="utf-8"))
    except (FileNotFoundError, json.JSONDecodeError):
        return True
    return cached.get("fingerprint") != fingerprint


def write_build_cache(cache_file: Path, fingerprint: str, artifact: Path) -> None:
    cache_file.parent.mkdir(parents=True, exist_ok=True)
    temporary = cache_file.with_suffix(".tmp")
    temporary.write_text(
        json.dumps({"fingerprint": fingerprint, "artifact": str(artifact)}, sort_keys=True) + "\n",
        encoding="utf-8",
    )
    os.replace(temporary, cache_file)
