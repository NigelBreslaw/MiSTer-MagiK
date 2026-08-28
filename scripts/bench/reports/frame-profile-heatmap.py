#!/usr/bin/env python3
# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

"""Render a dirty-region heatmap SVG from a frame-profile TSV.

Usage:
  scripts/bench/reports/frame-profile-heatmap.py /tmp/frames.tsv /tmp/heatmap.svg
"""

from __future__ import annotations

import argparse
import html
from pathlib import Path

from frame_profile_schema import int_field, read_rows


def add_rect(
    grid: list[list[int]],
    x0: int,
    y0: int,
    x1: int,
    y1: int,
    surface_w: int,
    surface_h: int,
) -> None:
    cols = len(grid[0])
    rows = len(grid)
    if x1 <= x0 or y1 <= y0:
        return
    gx0 = max(0, min(cols - 1, x0 * cols // surface_w))
    gx1 = max(0, min(cols - 1, (x1 - 1) * cols // surface_w))
    gy0 = max(0, min(rows - 1, y0 * rows // surface_h))
    gy1 = max(0, min(rows - 1, (y1 - 1) * rows // surface_h))
    for gy in range(gy0, gy1 + 1):
        for gx in range(gx0, gx1 + 1):
            grid[gy][gx] += 1


def color(value: int, max_value: int) -> str:
    if value <= 0 or max_value <= 0:
        return "#f9fafb"
    t = value / max_value
    # White -> cyan -> orange -> red.
    if t < 0.5:
        k = t / 0.5
        r = round(249 * (1 - k) + 34 * k)
        g = round(250 * (1 - k) + 211 * k)
        b = round(252 * (1 - k) + 238 * k)
    else:
        k = (t - 0.5) / 0.5
        r = round(34 * (1 - k) + 220 * k)
        g = round(211 * (1 - k) + 38 * k)
        b = round(238 * (1 - k) + 38 * k)
    return f"#{r:02x}{g:02x}{b:02x}"


def svg_text(x: float, y: float, text: str, size: int = 12) -> str:
    return (
        f'<text x="{x:.1f}" y="{y:.1f}" font-size="{size}" '
        f'font-family="system-ui, sans-serif" fill="#111827">{html.escape(text)}</text>'
    )


def render_heatmap(
    rows: list[dict[str, str]], cols: int, grid_rows: int, title: str
) -> str:
    surface_w = max(1, max(int_field(row, "present_x1") for row in rows))
    surface_h = max(1, max(int_field(row, "present_y1") for row in rows))
    grid = [[0 for _ in range(cols)] for _ in range(grid_rows)]
    rects = 0
    for row in rows:
        x0 = int_field(row, "present_x0")
        y0 = int_field(row, "present_y0")
        x1 = int_field(row, "present_x1")
        y1 = int_field(row, "present_y1")
        if x1 > x0 and y1 > y0:
            rects += 1
            add_rect(grid, x0, y0, x1, y1, surface_w, surface_h)

    max_value = max(max(row) for row in grid) if grid else 0
    cell = 10
    margin_l = 24
    margin_t = 62
    width = margin_l * 2 + cols * cell
    height = margin_t + grid_rows * cell + 40
    parts = [
        f'<svg xmlns="http://www.w3.org/2000/svg" width="{width}" height="{height}" viewBox="0 0 {width} {height}">',
        '<rect width="100%" height="100%" fill="#ffffff"/>',
        svg_text(18, 26, title, 16),
        svg_text(
            18,
            44,
            f"{rects} presented rects · surface {surface_w}x{surface_h} · max bucket hits {max_value}",
            11,
        ),
    ]
    for gy, row in enumerate(grid):
        for gx, value in enumerate(row):
            x = margin_l + gx * cell
            y = margin_t + gy * cell
            parts.append(
                f'<rect x="{x}" y="{y}" width="{cell}" height="{cell}" fill="{color(value, max_value)}">'
                f"<title>x={gx} y={gy} hits={value}</title></rect>"
            )
    parts.append(
        f'<rect x="{margin_l}" y="{margin_t}" width="{cols * cell}" height="{grid_rows * cell}" fill="none" stroke="#111827"/>'
    )
    parts.append("</svg>")
    return "\n".join(parts)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("input", type=Path)
    parser.add_argument("output", type=Path)
    parser.add_argument("--cols", type=int, default=96)
    parser.add_argument("--rows", type=int, default=54)
    parser.add_argument("--title", default=None)
    args = parser.parse_args()

    rows = read_rows(args.input)
    title = args.title or args.input.name
    args.output.write_text(
        render_heatmap(rows, args.cols, args.rows, title), encoding="utf-8"
    )
    print(f"wrote {args.output} ({len(rows)} frames)")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
