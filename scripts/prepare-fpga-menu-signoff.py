#!/usr/bin/env python3
# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

"""Apply the canonical deterministic Menu settings used by every FPGA lane."""

from __future__ import annotations

import argparse
from pathlib import Path


def replace_exactly_once(path: Path, before: str, after: str) -> None:
    source = path.read_text(encoding="utf-8")
    if source.count(before) != 1:
        raise SystemExit(f"expected exactly one {before!r} in {path}")
    path.write_text(source.replace(before, after, 1), encoding="utf-8")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("menu_root", type=Path)
    args = parser.parse_args()

    root = args.menu_root.resolve()
    qsf = root / "menu.qsf"
    sdc = root / "sys/sys_top.sdc"
    if not qsf.is_file() or not sdc.is_file():
        parser.error(f"not a Menu_MiSTer checkout: {root}")

    replace_exactly_once(
        sdc,
        "set_clock_groups -exclusive",
        "set_clock_groups -asynchronous",
    )
    replace_exactly_once(
        qsf,
        "set_global_assignment -name NUM_PARALLEL_PROCESSORS ALL",
        "set_global_assignment -name NUM_PARALLEL_PROCESSORS 4",
    )

    source = qsf.read_text(encoding="utf-8")
    for assignment in ("PARALLEL_SYNTHESIS", "AUTO_PARALLEL_SYNTHESIS"):
        if f"set_global_assignment -name {assignment}" in source:
            raise SystemExit(f"unexpected existing {assignment} assignment in {qsf}")
    qsf.write_text(
        source.rstrip("\n")
        + "\n# Canonical deterministic MiSTer MagiK signoff settings.\n"
        + "set_global_assignment -name PARALLEL_SYNTHESIS OFF\n"
        + "set_global_assignment -name AUTO_PARALLEL_SYNTHESIS OFF\n",
        encoding="utf-8",
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
