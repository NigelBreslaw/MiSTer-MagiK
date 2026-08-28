#!/usr/bin/env python3
# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

"""Generate an HTML index for frame profile report artifacts."""

from __future__ import annotations

import argparse
import html
from pathlib import Path


def row_for_report(path: Path, root: Path) -> str:
    stem = path.name.removesuffix("-report.html")
    siblings = {
        "report": path,
        "chart": root / f"{stem}-chart.svg",
        "heatmap": root / f"{stem}-heatmap.svg",
        "frames": root / f"{stem}-frames.tsv",
        "trace": root / f"{stem}-trace.json",
    }
    links = []
    for label, artifact in siblings.items():
        if artifact.exists():
            links.append(f'<a href="{html.escape(artifact.name)}">{label}</a>')
    return (
        "<tr>"
        f"<td>{html.escape(stem)}</td>"
        f"<td>{html.escape(path.stat().st_mtime_ns.__str__())}</td>"
        f"<td>{' · '.join(links)}</td>"
        "</tr>"
    )


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "directory", type=Path, nargs="?", default=Path("build/frame-profiles")
    )
    parser.add_argument("--output", type=Path, default=None)
    args = parser.parse_args()

    root = args.directory
    output = args.output or root / "index.html"
    reports = sorted(
        root.glob("*-report.html"), key=lambda p: p.stat().st_mtime, reverse=True
    )
    rows = "\n".join(row_for_report(path, root) for path in reports)
    doc = f"""<!doctype html>
<html lang="en">
<meta charset="utf-8">
<title>Frame Profile Reports</title>
<style>
body {{ font-family: system-ui, sans-serif; margin: 24px; color: #111827; }}
table {{ border-collapse: collapse; }}
th, td {{ border: 1px solid #d1d5db; padding: 6px 10px; }}
a {{ color: #2563eb; }}
</style>
<h1>Frame Profile Reports</h1>
<p>Directory: <code>{html.escape(str(root))}</code></p>
<table>
<thead><tr><th>Label</th><th>mtime ns</th><th>Artifacts</th></tr></thead>
<tbody>
{rows}
</tbody>
</table>
</html>
"""
    output.write_text(doc, encoding="utf-8")
    print(f"wrote {output} ({len(reports)} reports)")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
