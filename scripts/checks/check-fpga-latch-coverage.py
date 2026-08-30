#!/usr/bin/env python3
# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

"""Require every reachable Verilator flow/branch point in the custom RTL to be hit."""

from __future__ import annotations

import argparse
import re
import sys
from pathlib import Path


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("annotation_dir", type=Path)
    parser.add_argument("--source", default="mister_magik_vblank_latch.sv")
    args = parser.parse_args()

    matches = list(args.annotation_dir.rglob(args.source))
    if len(matches) != 1:
        print(
            f"expected one annotated {args.source}, found {len(matches)}",
            file=sys.stderr,
        )
        raise SystemExit(1)
    annotations = matches[0].read_text().splitlines()
    # Verilator 5.050 reports the false arm of this comparison as an uncovered
    # branch. The enclosing `word_index < 4'd15` test is false and word_index is
    # four bits wide, so `word_index == 4'd15` is then an identity: its false
    # arm has no possible stimulus. Older Verilator releases do not annotate it.
    unreachable_latch_branch = "else if(word_index == 4'd15)"
    incomplete = [
        line
        for line in annotations
        if line.startswith(("%", "~"))
        and not (
            args.source == "mister_magik_vblank_latch.sv"
            and unreachable_latch_branch in line
        )
    ]
    covered = [line for line in annotations if re.match(r"^[ +]?\d+", line)]
    if not covered:
        print(f"no coverage points found for {args.source}", file=sys.stderr)
        raise SystemExit(1)
    if incomplete:
        print("incomplete custom RTL line/branch coverage:", file=sys.stderr)
        for line in incomplete:
            print(line, file=sys.stderr)
        raise SystemExit(1)
    print(f"custom RTL line/branch coverage complete: {len(covered)} annotated points")


if __name__ == "__main__":
    main()
