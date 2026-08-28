#!/usr/bin/env python3
# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

"""Describe the Distribution_MiSTer alternatives omitted from its updater DB."""

from __future__ import annotations

import hashlib
import json
import sys
from pathlib import Path


def main() -> int:
    if len(sys.argv) != 3:
        raise SystemExit("usage: build-alternatives-database.py SOURCE OUTPUT")
    source = Path(sys.argv[1]).resolve()
    output = Path(sys.argv[2])
    alternatives = source / "_Arcade" / "_alternatives"
    files: dict[str, dict[str, object]] = {}
    for path in sorted(alternatives.rglob("*.mra")):
        data = path.read_bytes()
        installed = path.relative_to(source).as_posix()
        files[installed] = {
            "hash": hashlib.md5(data, usedforsecurity=False).hexdigest(),
            "size": len(data),
        }
    if not files:
        raise SystemExit(f"no MRA alternatives found under {alternatives}")
    output.parent.mkdir(parents=True, exist_ok=True)
    output.write_text(
        json.dumps({"files": files}, sort_keys=True, separators=(",", ":")) + "\n",
        encoding="utf-8",
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
