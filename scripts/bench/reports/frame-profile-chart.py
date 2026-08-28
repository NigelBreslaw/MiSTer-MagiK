#!/usr/bin/env python3
# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

"""Render a frame-profile TSV as a stacked SVG bar chart.

Usage:
  scripts/bench/reports/frame-profile-chart.py /tmp/frames.tsv /tmp/frames.svg

The input is the `MISTER_PROFILE_FILE` TSV written by `mister-magik-fb`.
No third-party packages are required.
"""

from __future__ import annotations

import argparse
import html
from pathlib import Path

from frame_profile_schema import int_field, read_rows

PHASES = [
    ("prepare_us", "prepare", "#6b7280"),
    ("anim_us", "anim", "#22c55e"),
    ("slint_render_us", "slint-render", "#2563eb"),
    ("custom_draw_us", "custom-draw", "#a855f7"),
    ("vsync_us", "vsync-wait", "#f59e0b"),
    ("video_recv_us", "video-recv", "#64748b"),
    ("video_decode_us", "video-decode", "#10b981"),
    ("video_scale_us", "video-scale", "#84cc16"),
    ("video_image_us", "video-image", "#6366f1"),
    ("video_blit_us", "video-blit", "#0ea5e9"),
    ("audio_decode_us", "audio-decode", "#f97316"),
    ("audio_resample_us", "audio-resample", "#fb7185"),
    ("audio_write_us", "audio-write", "#b45309"),
    ("cached_present_us", "cached-present", "#06b6d4"),
    ("hidden_compose_us", "hidden-compose", "#f43f5e"),
    ("arcade_list_present_us", "arcade-list-present", "#ef4444"),
]

DETAIL_PHASES = [
    ("prepare_us", "prepare", "#6b7280"),
    ("anim_us", "anim", "#22c55e"),
    ("slint_render_us", "slint-render", "#2563eb"),
    ("arcade_list_update_us", "list-update", "#16a34a"),
    ("preview_blit_us", "preview-blit", "#9333ea"),
    ("effect_label_us", "effect-label", "#db2777"),
    ("custom_draw_other_us", "custom-other", "#c084fc"),
    ("vsync_us", "vsync-wait", "#f59e0b"),
    ("video_recv_us", "video-recv", "#64748b"),
    ("video_decode_us", "video-decode", "#10b981"),
    ("video_scale_us", "video-scale", "#84cc16"),
    ("video_image_us", "video-image", "#6366f1"),
    ("video_blit_us", "video-blit", "#0ea5e9"),
    ("audio_decode_us", "audio-decode", "#f97316"),
    ("audio_resample_us", "audio-resample", "#fb7185"),
    ("audio_write_us", "audio-write", "#b45309"),
    ("cached_present_us", "cached-present", "#06b6d4"),
    ("hidden_preview_compose_us", "hidden-preview", "#fb7185"),
    ("hidden_arcade_compose_us", "hidden-arcade", "#e11d48"),
    ("arcade_list_present_us", "arcade-list-present", "#ef4444"),
    ("fb_present_other_us", "present-other", "#94a3b8"),
]


def has_detail_phases(rows: list[dict[str, str]]) -> bool:
    if not rows:
        return False
    keys = rows[0].keys()
    return {"arcade_list_update_us", "preview_blit_us", "effect_label_us"}.issubset(
        keys
    ) or {"video_decode_us", "audio_write_us"}.issubset(keys)


def phase_value(row: dict[str, str], key: str) -> int:
    if key == "custom_draw_other_us":
        known = (
            int_field(row, "arcade_list_update_us")
            + int_field(row, "preview_blit_us")
            + int_field(row, "effect_label_us")
        )
        return max(0, int_field(row, "custom_draw_us") - known)
    if key == "fb_present_other_us":
        hidden_compose = int_field(row, "hidden_compose_us")
        compose_present = hidden_compose or (
            int_field(row, "direct_preview_present_us")
            + int_field(row, "arcade_list_present_us")
        )
        known = int_field(row, "cached_present_us") + compose_present
        return max(0, int_field(row, "fb_present_us") - known)
    return int_field(row, key)


def svg_text(
    x: float, y: float, text: str, size: int = 12, anchor: str = "start"
) -> str:
    return (
        f'<text x="{x:.1f}" y="{y:.1f}" font-size="{size}" '
        f'font-family="system-ui, sans-serif" text-anchor="{anchor}" '
        f'fill="#111827">{html.escape(text)}</text>'
    )


def render_svg(rows: list[dict[str, str]], title: str, width: int, height: int) -> str:
    phases = DETAIL_PHASES if has_detail_phases(rows) else PHASES
    margin_l = 62
    margin_r = 24
    margin_t = 54
    margin_b = 72
    plot_w = width - margin_l - margin_r
    plot_h = height - margin_t - margin_b
    frame_count = max(1, len(rows))
    budget_us = 16_667
    max_us = max(
        budget_us,
        max((int_field(row, "wall_us") or int_field(row, "phases_us")) for row in rows)
        if rows
        else 0,
    )
    # Leave headroom for labels and occasional slow frames.
    max_us = int(max_us * 1.08)
    bar_gap = 1
    bar_w = max(1.0, (plot_w - (frame_count - 1) * bar_gap) / frame_count)
    scale = plot_h / max_us

    parts: list[str] = [
        f'<svg xmlns="http://www.w3.org/2000/svg" width="{width}" height="{height}" viewBox="0 0 {width} {height}">',
        '<rect width="100%" height="100%" fill="#ffffff"/>',
        svg_text(18, 26, title, 16),
        svg_text(
            18,
            44,
            f"{len(rows)} frames · stacked frame phases · 16.667ms budget line",
            11,
        ),
    ]

    # Axes and budget line.
    x0 = margin_l
    y0 = margin_t + plot_h
    parts.append(
        f'<line x1="{x0}" y1="{margin_t}" x2="{x0}" y2="{y0}" stroke="#9ca3af"/>'
    )
    parts.append(
        f'<line x1="{x0}" y1="{y0}" x2="{margin_l + plot_w}" y2="{y0}" stroke="#9ca3af"/>'
    )
    budget_y = y0 - budget_us * scale
    parts.append(
        f'<line x1="{x0}" y1="{budget_y:.1f}" x2="{margin_l + plot_w}" y2="{budget_y:.1f}" '
        'stroke="#dc2626" stroke-dasharray="4 4"/>'
    )
    parts.append(svg_text(8, budget_y + 4, "16.667ms", 10))
    for tick_us in range(0, max_us + 1, 5000):
        y = y0 - tick_us * scale
        parts.append(
            f'<line x1="{x0 - 4}" y1="{y:.1f}" x2="{x0}" y2="{y:.1f}" stroke="#9ca3af"/>'
        )
        parts.append(svg_text(x0 - 8, y + 4, f"{tick_us // 1000}ms", 10, "end"))

    for idx, row in enumerate(rows):
        x = margin_l + idx * (bar_w + bar_gap)
        y = y0
        tooltip_bits = [f"frame {row.get('frame', idx)}"]
        for key, label, color in phases:
            value = phase_value(row, key)
            if value <= 0:
                continue
            h = max(0.5, value * scale)
            y -= h
            tooltip_bits.append(f"{label}: {value}us")
            parts.append(
                f'<rect x="{x:.1f}" y="{y:.1f}" width="{bar_w:.1f}" height="{h:.1f}" fill="{color}">'
                f"<title>{html.escape(' · '.join(tooltip_bits))}</title></rect>"
            )
        if idx % max(1, frame_count // 8) == 0:
            parts.append(svg_text(x + bar_w / 2, y0 + 16, str(idx), 9, "middle"))

    legend_x = margin_l
    legend_y = height - 44
    for key, label, color in phases:
        parts.append(
            f'<rect x="{legend_x}" y="{legend_y}" width="10" height="10" fill="{color}"/>'
        )
        parts.append(svg_text(legend_x + 14, legend_y + 10, label, 10))
        legend_x += 112
        if legend_x > width - 140:
            legend_x = margin_l
            legend_y += 18

    parts.append("</svg>")
    return "\n".join(parts)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("input", type=Path, help="frame profile TSV")
    parser.add_argument("output", type=Path, help="output SVG")
    parser.add_argument("--max-frames", type=int, default=240)
    parser.add_argument("--width", type=int, default=1200)
    parser.add_argument("--height", type=int, default=520)
    parser.add_argument("--title", default=None)
    args = parser.parse_args()

    rows = read_rows(args.input, max_rows=args.max_frames)
    title = args.title or args.input.name
    args.output.write_text(
        render_svg(rows, title, args.width, args.height), encoding="utf-8"
    )
    print(f"wrote {args.output} ({len(rows)} frames)")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
