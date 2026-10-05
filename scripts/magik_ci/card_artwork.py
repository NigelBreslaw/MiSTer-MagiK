# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

"""Validate the runtime card pack shared by ZIP and Downloader installs."""

import json
import runpy
from pathlib import Path

from .common import sha256_file

RELATIVE_PATH = "assets/ui/launcher-cards"
# Load the pure host contract without depending on either Python package path.
_contract = runpy.run_path(
    str(Path(__file__).resolve().parents[2] / "magik/host/magik/artwork_manifest.py")
)
files_for = _contract["files_for"]
SOURCE_BYTES = _contract["SOURCE_BYTES"]
ROOT_KEYS = _contract["ROOT_KEYS"]


def validate(root: Path) -> set[str]:
    payload = (root / "index.json").read_bytes()
    if len(payload) > 128 * 1024:
        raise ValueError("oversized card artwork index")
    index = json.loads(payload)
    files = files_for(index)
    for name, (digest, size) in files.items():
        path = root / name
        if path.is_symlink() or not path.is_file() or path.stat().st_size != size:
            raise ValueError(f"invalid card artwork file: {name}")
        if sha256_file(path) != digest:
            raise ValueError(f"card artwork checksum mismatch: {name}")
    return {"index.json", *files}
