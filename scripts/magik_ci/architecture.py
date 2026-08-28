from __future__ import annotations

import json
import re
from argparse import Namespace
from dataclasses import asdict, dataclass
from pathlib import Path
from typing import Any, cast

from .common import git

SCHEMA = "mister-magik-architecture-report-v1"


@dataclass(frozen=True)
class Hotspot:
    owner_id: str
    path: str
    intended_destination: str


HOTSPOTS = (
    Hotspot(
        "launcher-runtime",
        "apps/mister/src/ui_runner/launcher_loop.rs",
        "P1 Decompose launcher state and frame phases",
    ),
    Hotspot(
        "host-workflows",
        "agent-cli/src/host/mod.rs",
        "P2-A typed host workflow modules",
    ),
    Hotspot(
        "desktop-app",
        "apps/desktop/src/main.rs",
        "P2 next-tier desktop ownership seams",
    ),
    Hotspot(
        "catalog-persistence",
        "crates/catalog/src/sqlite_catalog.rs",
        "P2-B characterization then P3 persistence split",
    ),
)


def _function(source: str) -> dict[str, object] | None:
    lines = source.splitlines()
    best: tuple[str, int] | None = None
    for index, line in enumerate(lines):
        match = re.search(r"(?:pub\s+)?(?:async\s+)?fn\s+([A-Za-z0-9_]+)", line)
        if not match:
            continue
        depth = 0
        opened = False
        end = index
        for offset, candidate in enumerate(lines[index:]):
            depth += candidate.count("{") - candidate.count("}")
            opened |= "{" in candidate
            end = index + offset
            if opened and depth <= 0:
                break
        if opened and (best is None or end - index + 1 > best[1]):
            best = (match.group(1), end - index + 1)
    return None if best is None else {"name": best[0], "lines": best[1]}


def report(repository: Path, base: str, head: str) -> dict[str, object]:
    base = git(repository, "rev-parse", base)
    head = git(repository, "rev-parse", head)
    diff = git(repository, "diff", "--numstat", base, head)
    changed: dict[str, int] = {}
    total = 0
    for line in diff.splitlines():
        parts = line.split("\t")
        if len(parts) == 3 and parts[0].isdigit():
            changed[parts[2]] = int(parts[0]) + int(parts[1])
            total += changed[parts[2]]
    hotspots: list[dict[str, object]] = []
    for hotspot in HOTSPOTS:
        source = git(repository, "show", f"{head}:{hotspot.path}", check=False)
        present = bool(source)
        hotspots.append(
            {
                **asdict(hotspot),
                "present": present,
                "file_lines": len(source.splitlines()),
                "largest_function": _function(source),
                "mutable_binding_count": source.count("let mut "),
                "direct_environment_read_count": source.count("env::var(")
                + source.count("env::var_os("),
                "public_module_count": sum(
                    line.lstrip().startswith("pub mod ") for line in source.splitlines()
                ),
                "changed_lines": changed.get(hotspot.path, 0),
                "change_concentration_basis_points": changed.get(hotspot.path, 0)
                * 10_000
                // total
                if total
                else 0,
            }
        )
    return {
        "schema": SCHEMA,
        "base": base,
        "head": head,
        "total_changed_lines": total,
        "advisory_only": True,
        "hotspots": hotspots,
    }


def execute(repository: Path, args: Namespace) -> None:
    value = report(repository, args.base, args.head)
    value_data = cast(dict[str, Any], value)
    if args.format == "markdown":
        lines = [
            f"# Architecture report ({value['base']}..{value['head']})",
            "",
            "Advisory only.",
            "",
            "| Owner | File | Changed lines |",
            "| --- | --- | ---: |",
        ]
        lines.extend(
            f"| {item['owner_id']} | `{item['path']}` | {item['changed_lines']} |"
            for item in cast(list[dict[str, Any]], value_data["hotspots"])
        )
        rendered = "\n".join(lines) + "\n"
    else:
        rendered = json.dumps(value, indent=2, sort_keys=True) + "\n"
    if args.output:
        args.output.write_text(rendered, encoding="utf-8")
    else:
        print(rendered, end="")
