#!/usr/bin/env python3
# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

"""Canonical loading and numeric semantics for frame-profile reports."""

from __future__ import annotations

import csv
from pathlib import Path

LEGACY_ALIASES = {"arcade_list_present_us": "overlay_present_us"}

CANONICAL_PHASES = [
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
    "direct_preview_present_us",
    "arcade_list_present_us",
    "video_recv_us",
    "video_decode_us",
    "video_scale_us",
    "video_image_us",
    "video_blit_us",
    "audio_decode_us",
    "audio_resample_us",
    "audio_write_us",
    "present_pixels",
    "present_bytes",
]


def int_field(row: dict[str, str], key: str) -> int:
    if key not in row and key in LEGACY_ALIASES:
        key = LEGACY_ALIASES[key]
    value = row.get(key, "")
    if value in (None, ""):
        return 0
    try:
        return int(float(value))
    except (TypeError, ValueError):
        return 0


def read_rows(path: Path, max_rows: int | None = None) -> list[dict[str, str]]:
    with path.open(newline="", encoding="utf-8") as source:
        reader = csv.DictReader(source, delimiter="\t")
        if not reader.fieldnames:
            raise ValueError(f"frame profile has no TSV header: {path}")
        if len(reader.fieldnames) != len(set(reader.fieldnames)):
            raise ValueError(f"frame profile has duplicate columns: {path}")
        rows = list(reader)
    for index, row in enumerate(rows, start=2):
        if None in row:
            raise ValueError(f"frame profile row {index} has extra columns: {path}")
    return rows if max_rows is None else rows[:max_rows]


def percentile(values: list[int], pct: float) -> int:
    if not values:
        return 0
    ordered = sorted(values)
    index = round((len(ordered) - 1) * pct / 100.0)
    return ordered[min(len(ordered) - 1, index)]


def phase_stats(rows: list[dict[str, str]], key: str) -> dict[str, int]:
    values = [int_field(row, key) for row in rows]
    if not values:
        return {"avg": 0, "p50": 0, "p95": 0, "p99": 0, "max": 0}
    return {
        "avg": sum(values) // len(values),
        "p50": percentile(values, 50),
        "p95": percentile(values, 95),
        "p99": percentile(values, 99),
        "max": max(values),
    }
