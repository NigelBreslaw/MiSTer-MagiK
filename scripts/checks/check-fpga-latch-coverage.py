#!/usr/bin/env python3
# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

"""Require every Verilator flow/branch point in the custom RTL to be hit."""

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
    incomplete = [line for line in annotations if line.startswith(("%", "~"))]
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
