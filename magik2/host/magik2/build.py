"""Probe input fingerprints; unrelated checkout state is intentionally ignored."""

from __future__ import annotations

import hashlib
import json
import os
import re
import subprocess
import time
from dataclasses import dataclass
from collections.abc import Callable, Iterable
from pathlib import Path

INPUT_NAMES = {"Cargo.toml", "Cargo.lock", "build.rs"}
INPUT_SUFFIXES = {".rs", ".slint"}


@dataclass(frozen=True)
class BuildResult:
    artifact: Path
    rebuilt: bool
    elapsed_ms: int


def relevant_inputs(probe_root: Path) -> list[Path]:
    inputs = {
        path
        for path in probe_root.rglob("*")
        if path.is_file() and (path.name in INPUT_NAMES or path.suffix in INPUT_SUFFIXES)
    }
    for document in tuple(path for path in inputs if path.suffix == ".slint"):
        for imported in re.findall(r'^\s*import\s+"([^"]+)"', document.read_text(encoding="utf-8"), re.MULTILINE):
            candidate = (document.parent / imported).resolve()
            if candidate.is_file():
                inputs.add(candidate)
    return sorted(inputs)


def source_fingerprint(probe_root: Path) -> str:
    digest = hashlib.sha256()
    for path in relevant_inputs(probe_root):
        try:
            name = path.relative_to(probe_root).as_posix()
        except ValueError:
            name = f"external/{path.name}"
        digest.update(name.encode() + b"\0")
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


def ensure_arm_probe(
    probe_root: Path,
    cache_file: Path,
    *,
    runner: Callable[..., subprocess.CompletedProcess[object]] = subprocess.run,
) -> BuildResult:
    """Build only when relevant probe inputs or its ARM artifact changed."""
    artifact = probe_root / "target" / "armv7-unknown-linux-gnueabihf" / "release" / "mister-magik2-probe"
    fingerprint = source_fingerprint(probe_root)
    started = time.monotonic()
    if artifact.is_file() and not needs_build(cache_file, fingerprint):
        return BuildResult(artifact, False, int((time.monotonic() - started) * 1_000))
    result = runner(
        [
            "container",
            "exec",
            "magik2-arm-build",
            "/bin/bash",
            "-lc",
            "cd /workspace/magik2/probe && cargo build --release --target armv7-unknown-linux-gnueabihf",
        ],
        check=False,
    )
    if result.returncode != 0 or not artifact.is_file():
        raise RuntimeError("MagiK 2 ARM probe build failed")
    write_build_cache(cache_file, fingerprint, artifact)
    return BuildResult(artifact, True, int((time.monotonic() - started) * 1_000))
