#!/usr/bin/env python3
"""Generate a self-contained HTML report for a frame-profile TSV."""

from __future__ import annotations

import argparse
import csv
import html
import subprocess
import sys
import tempfile
from pathlib import Path


PHASES = [
    "wall_us",
    "prepare_us",
    "anim_us",
    "slint_render_us",
    "custom_draw_us",
    "vsync_us",
    "fb_present_us",
    "cached_present_us",
    "direct_preview_present_us",
    "arcade_list_present_us",
    "video_decode_us",
    "video_scale_us",
    "video_recv_us",
    "video_image_us",
    "video_blit_us",
    "audio_decode_us",
    "audio_resample_us",
    "audio_write_us",
    "present_pixels",
    "present_bytes",
]


def int_field(row: dict[str, str], key: str) -> int:
    if key == "arcade_list_present_us" and key not in row:
        key = "overlay_present_us"
    try:
        return int(float(row.get(key, "") or 0))
    except ValueError:
        return 0


def percentile(values: list[int], pct: float) -> int:
    if not values:
        return 0
    values = sorted(values)
    idx = round((len(values) - 1) * pct / 100.0)
    return values[min(len(values) - 1, idx)]


def read_rows(path: Path) -> list[dict[str, str]]:
    with path.open(newline="") as f:
        return list(csv.DictReader(f, delimiter="\t"))


def command_output(args: list[str]) -> str:
    return subprocess.check_output(args, text=True)


def phase_table(rows: list[dict[str, str]]) -> str:
    out = ["<table><thead><tr><th>Metric</th><th>Avg</th><th>p50</th><th>p95</th><th>p99</th><th>Max</th></tr></thead><tbody>"]
    for phase in PHASES:
        values = [int_field(row, phase) for row in rows]
        if not values:
            continue
        out.append(
            "<tr>"
            f"<td>{html.escape(phase)}</td>"
            f"<td>{sum(values) // len(values)}</td>"
            f"<td>{percentile(values, 50)}</td>"
            f"<td>{percentile(values, 95)}</td>"
            f"<td>{percentile(values, 99)}</td>"
            f"<td>{max(values)}</td>"
            "</tr>"
        )
    out.append("</tbody></table>")
    return "\n".join(out)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("input", type=Path)
    parser.add_argument("output", type=Path)
    parser.add_argument("--title", default=None)
    parser.add_argument("--trace", type=Path, default=None)
    args = parser.parse_args()

    rows = read_rows(args.input)
    title = args.title or args.input.name
    here = Path(__file__).resolve().parent
    with tempfile.NamedTemporaryFile(suffix=".svg") as tmp:
        command_output(
            [
                sys.executable,
                str(here / "frame-profile-chart.py"),
                str(args.input),
                tmp.name,
                "--max-frames",
                "180",
                "--title",
                title,
            ]
        )
        chart_svg = Path(tmp.name).read_text(encoding="utf-8")
    slow = command_output(
        [sys.executable, str(here / "frame-profile-slow-frames.py"), str(args.input), "--limit", "8"]
    )
    trace_link = ""
    if args.trace:
        trace_link = f'<p>Trace: <code>{html.escape(str(args.trace))}</code></p>'
    html_doc = f"""<!doctype html>
<html lang="en">
<meta charset="utf-8">
<title>{html.escape(title)}</title>
<style>
body {{ font-family: system-ui, sans-serif; margin: 24px; color: #111827; }}
pre {{ background: #f3f4f6; padding: 12px; overflow: auto; }}
table {{ border-collapse: collapse; margin: 16px 0; }}
th, td {{ border: 1px solid #d1d5db; padding: 4px 8px; text-align: right; }}
th:first-child, td:first-child {{ text-align: left; }}
code {{ background: #f3f4f6; padding: 2px 4px; }}
</style>
<h1>{html.escape(title)}</h1>
<p>Input: <code>{html.escape(str(args.input))}</code> · frames: {len(rows)}</p>
{trace_link}
<h2>Stacked Frame Phases</h2>
{chart_svg}
<h2>Phase Stats</h2>
{phase_table(rows)}
<h2>Slow Frames</h2>
<pre>{html.escape(slow)}</pre>
</html>
"""
    args.output.write_text(html_doc, encoding="utf-8")
    print(f"wrote {args.output} ({len(rows)} frames)")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
