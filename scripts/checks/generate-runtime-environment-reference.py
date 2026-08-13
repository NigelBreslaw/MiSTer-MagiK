#!/usr/bin/env python3
# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

import argparse
from pathlib import Path
import sys
import tomllib


ROOT = Path(__file__).resolve().parents[2]
DEFAULT_REGISTRY = ROOT / "apps/mister/config/runtime-environment.toml"
DEFAULT_OUTPUT = ROOT / "docs/reference/mister-runtime-environment.md"


def render(registry: dict) -> str:
    baseline = registry["baseline"]
    lines = [
        "# MiSTer runtime environment reference",
        "",
        "<!-- Generated from apps/mister/config/runtime-environment.toml. Do not edit. -->",
        "",
        (
            f"Registry format: `{registry['format']}`. Baseline: "
            f"{baseline['literal_occurrences']} literal occurrences, "
            f"{baseline['unique_names']} owned names, "
            f"{baseline['external_build_names']} external/build-time names."
        ),
        "",
        "| Name | Classification | Shape | Default behavior | Visibility | Owner |",
        "|---|---|---|---|---|---|",
    ]
    for control in sorted(registry["control"], key=lambda value: value["name"]):
        default = control["default_behavior"].replace("|", "\\|")
        lines.append(
            f"| `{control['name']}` | {control['classification']} | "
            f"{control['value_shape']} | {default} | {control['visibility']} | "
            f"`{control['owner']}` |"
        )
    return "\n".join(lines) + "\n"


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--registry", type=Path, default=DEFAULT_REGISTRY)
    parser.add_argument("--output", type=Path, default=DEFAULT_OUTPUT)
    mode = parser.add_mutually_exclusive_group()
    mode.add_argument("--check", action="store_true")
    mode.add_argument("--stdout", action="store_true")
    args = parser.parse_args()

    reference = render(tomllib.loads(args.registry.read_text(encoding="utf-8")))
    if args.stdout:
        sys.stdout.write(reference)
        return 0
    if args.check:
        if not args.output.is_file() or args.output.read_text(encoding="utf-8") != reference:
            print(f"stale generated reference: {args.output}", file=sys.stderr)
            return 1
        return 0
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(reference, encoding="utf-8")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
