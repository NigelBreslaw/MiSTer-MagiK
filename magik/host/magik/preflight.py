"""Fail before expensive preparation when conservative disk headroom is absent."""

from pathlib import Path
import shutil


def require_space(path: Path, required: int, operation: str):
    existing = path.resolve()
    while not existing.exists():
        existing = existing.parent
    free = shutil.disk_usage(existing).free
    if free < required:
        raise RuntimeError(
            f"{operation}: {path.resolve()} needs {required / 1024**3:.1f} GiB free; "
            f"only {free / 1024**3:.1f} GiB available. Free space and rerun; no automatic deletion."
        )
