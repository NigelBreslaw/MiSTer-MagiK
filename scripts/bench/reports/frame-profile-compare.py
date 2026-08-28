#!/usr/bin/env python3
# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

"""Compare two frame-profile TSVs phase by phase.

Usage:
  scripts/bench/reports/frame-profile-compare.py before.tsv after.tsv
"""

from __future__ import annotations

import argparse
from pathlib import Path

from frame_profile_schema import phase_stats, read_rows

PHASES = [
    "wall_us",
    "prepare_us",
    "anim_us",
    "slint_render_us",
    "custom_draw_us",
    "vsync_us",
    "fb_present_us",
    "cached_present_us",
    "hidden_compose_us",
    "hidden_preview_compose_us",
    "hidden_arcade_compose_us",
    "arcade_list_present_us",
    "present_pixels",
    "present_bytes",
]


def stats(rows: list[dict[str, str]], key: str) -> dict[str, int]:
    return phase_stats(rows, key)


def fmt_delta(delta: int) -> str:
    if delta > 0:
        return f"+{delta}"
    return str(delta)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("before", type=Path)
    parser.add_argument("after", type=Path)
    args = parser.parse_args()

    before = read_rows(args.before)
    after = read_rows(args.after)
    print(f"before={args.before} frames={len(before)}")
    print(f"after ={args.after} frames={len(after)}")
    print(
        "metric\tbefore_avg\tafter_avg\tdelta_avg\tbefore_p50\tafter_p50\tdelta_p50\tbefore_p95\tafter_p95\tdelta_p95"
    )
    for phase in PHASES:
        b = stats(before, phase)
        a = stats(after, phase)
        print(
            f"{phase}\t{b['avg']}\t{a['avg']}\t{fmt_delta(a['avg'] - b['avg'])}"
            f"\t{b['p50']}\t{a['p50']}\t{fmt_delta(a['p50'] - b['p50'])}"
            f"\t{b['p95']}\t{a['p95']}\t{fmt_delta(a['p95'] - b['p95'])}"
        )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
