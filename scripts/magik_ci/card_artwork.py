# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

"""Validate the runtime card pack shared by ZIP and Downloader installs."""

import json
import re
from pathlib import Path

from .common import sha256_file

RELATIVE_PATH = "assets/ui/launcher-cards"
SOURCE_BYTES = 360 * 504 * 3
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


def validate(root: Path) -> set[str]:
    payload = (root / "index.json").read_bytes()
    if len(payload) > 128 * 1024:
        raise ValueError("oversized card artwork index")
    index = json.loads(payload)
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
        if not re.fullmatch(
            r"[A-Za-z0-9][A-Za-z0-9_.-]*\.rgb888", name
        ) or not re.fullmatch(r"[0-9a-f]{64}", digest):
            raise ValueError("invalid card artwork source")
        if name in files and files[name] != digest:
            raise ValueError("conflicting card artwork checksums")
        files[name] = digest
    for name, digest in files.items():
        path = root / name
        if (
            path.is_symlink()
            or not path.is_file()
            or path.stat().st_size != SOURCE_BYTES
        ):
            raise ValueError(f"invalid card artwork file: {name}")
        if sha256_file(path) != digest:
            raise ValueError(f"card artwork checksum mismatch: {name}")
    return {"index.json", *files}
