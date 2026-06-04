#!/usr/bin/env python3
"""Summarize a MISTER_PROFILE_FILE TSV (per-frame phase timings)."""
from __future__ import annotations

import csv
import sys
from pathlib import Path

BUDGET_US = 16_667


def pct(sorted_vals: list[int], p: float) -> int:
    if not sorted_vals:
        return 0
    idx = round((len(sorted_vals) - 1) * p / 100.0)
    return sorted_vals[max(0, min(idx, len(sorted_vals) - 1))]


def main() -> int:
    if len(sys.argv) != 2:
        print(f"usage: {sys.argv[0]} <frame-profile.tsv>", file=sys.stderr)
        return 2
    path = Path(sys.argv[1])
    rows: list[dict[str, str]] = []
    with path.open(newline="") as f:
        reader = csv.DictReader(f, delimiter="\t")
        for row in reader:
            rows.append(row)
    if not rows:
        print(f"{path}: empty", file=sys.stderr)
        return 1

    def col(name: str) -> list[int]:
        return [int(r[name]) for r in rows]

    wall = sorted(col("wall_us"))
    phases = sorted(col("phases_us"))
    anim = col("anim_us")
    render = col("render_us")
    vsync = col("vsync_us")
    copy_ = col("copy_us")
    n = len(wall)
    over = sum(1 for w in wall if w >= BUDGET_US)

    def line(label: str, vals: list[int]) -> None:
        s = sorted(vals)
        avg = sum(s) // len(s)
        print(
            f"  {label:10} min={s[0]:6} p50={pct(s, 50):6} p95={pct(s, 95):6} "
            f"p99={pct(s, 99):6} max={s[-1]:6} avg={avg:6}"
        )

    print(f"=== {path.name} ({n} frames) ===")
    line("wall_us", wall)
    line("phases_us", phases)
    line("anim_us", anim)
    line("render_us", render)
    line("vsync_us", vsync)
    line("copy_us", copy_)
    print(f"  frames >= {BUDGET_US}us: {over} ({100.0 * over / n:.2f}%)")
    print(f"  avg fps (wall): {1_000_000 / (sum(wall) / n):.1f}")

    for phase, vals in [
        ("anim", anim),
        ("render", render),
        ("vsync", vsync),
        ("copy", copy_),
    ]:
        slow = sum(
            1
            for r in rows
            if int(r["wall_us"]) >= BUDGET_US
            and max(int(r["anim_us"]), int(r["render_us"]), int(r["vsync_us"]), int(r["copy_us"]))
            == int(r[f"{phase}_us" if phase != "copy" else "copy_us"])
        )
        if over:
            print(f"  slow-frame max phase = {phase}: {slow}/{over}")

    return 0


if __name__ == "__main__":
    raise SystemExit(main())
