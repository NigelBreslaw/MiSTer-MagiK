#!/usr/bin/env python3
# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

"""Regenerate/annotate the baked core launch manifest from host evidence.

The checked-in manifest is still curated: this tool does not guess new launch
rules from arbitrary folders. It preserves the existing row shape and adds
diagnostic-only evidence fields when it can observe matching source files or
installed cores.
"""

from __future__ import annotations

import argparse
import json
import os
import re
import subprocess
import sys
from dataclasses import dataclass
from pathlib import Path
from typing import Any


@dataclass(frozen=True)
class ObservedCore:
    core_id: str
    path: str
    size: int
    mtime: int


def canonical_core_id(name: str) -> str:
    stem = Path(name).stem
    match = re.match(r"^(?P<core>.+)_[0-9]{8}$", stem)
    return match.group("core") if match else stem


def repo_root() -> Path:
    return Path(__file__).resolve().parents[2]


def default_main_dir(root: Path) -> Path:
    return Path(os.environ.get("MISTER_MAIN_DIR", root.parent / "Main_MiSTer"))


def read_json(path: Path) -> dict[str, Any]:
    with path.open("r", encoding="utf-8") as fh:
        return json.load(fh)


def core_names_for_row(row: dict[str, Any]) -> list[str]:
    names = [str(row.get("core_name", ""))]
    names.extend(str(name) for name in row.get("compatible_core_names", []))
    return [name for name in names if name]


def source_locations(main_dir: Path, row: dict[str, Any]) -> list[str]:
    if not main_dir.is_dir():
        return []
    needles = {
        row.get("core_path", ""),
        *core_names_for_row(row),
        *row.get("game_dirs", []),
        *row.get("extensions", []),
    }
    needles = {needle for needle in needles if needle}
    matches: list[str] = []
    for path in main_dir.rglob("*"):
        if len(matches) >= 12:
            break
        if not path.is_file():
            continue
        if path.suffix.lower() not in {
            ".cpp",
            ".h",
            ".hpp",
            ".c",
            ".ini",
            ".txt",
            ".md",
        }:
            continue
        try:
            text = path.read_text(encoding="utf-8", errors="ignore")
        except OSError:
            continue
        lower = text.lower()
        if any(needle.lower() in lower for needle in needles):
            matches.append(str(path.relative_to(main_dir)))
    return matches


def observed_cores_from_fixture(path: Path) -> list[ObservedCore]:
    cores: list[ObservedCore] = []
    for line in path.read_text(encoding="utf-8").splitlines():
        parts = line.split("\t")
        if len(parts) < 3:
            continue
        size = int(parts[0])
        mtime = int(parts[1])
        core_path = parts[2]
        cores.append(
            ObservedCore(
                canonical_core_id(Path(core_path).name),
                core_path,
                size,
                mtime,
            )
        )
    return cores


def observed_cores_from_device(agent: Path) -> list[ObservedCore]:
    result = subprocess.run(
        [str(agent), "device", "catalog", "cores"],
        check=True,
        text=True,
        capture_output=True,
    )
    cores: list[ObservedCore] = []
    for line in result.stdout.splitlines():
        parts = line.split("\t")
        if len(parts) < 3:
            continue
        try:
            size = int(float(parts[0]))
            mtime = int(float(parts[1]))
        except ValueError:
            continue
        core_path = parts[2]
        if Path(core_path).name.startswith("._"):
            continue
        cores.append(
            ObservedCore(
                canonical_core_id(Path(core_path).name),
                core_path,
                size,
                mtime,
            )
        )
    return cores


def annotate_manifest(
    manifest: dict[str, Any],
    main_dir: Path,
    observed_cores: list[ObservedCore],
) -> dict[str, Any]:
    by_core: dict[str, list[ObservedCore]] = {}
    for core in observed_cores:
        by_core.setdefault(core.core_id.lower(), []).append(core)

    out = dict(manifest)
    out["generated_by"] = "scripts/media/harvest-core-launch-manifest.py"
    out["source_main_dir"] = str(main_dir)
    rows = []
    for row in manifest.get("profiles", []):
        annotated = dict(row)
        observed = {
            core.path: core
            for core_name in core_names_for_row(row)
            for core in by_core.get(core_name.lower(), [])
        }.values()
        annotated["source_locations"] = source_locations(main_dir, row)
        annotated["observed_cores"] = [
            {
                "path": core.path,
                "size": core.size,
                "mtime": core.mtime,
            }
            for core in sorted(observed, key=lambda item: item.path.lower())
        ]
        rows.append(annotated)
    out["profiles"] = rows
    return out


def parse_args() -> argparse.Namespace:
    root = repo_root()
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--manifest",
        type=Path,
        default=root / "crates/catalog/data/core_launch_manifest.json",
        help="existing curated manifest to annotate",
    )
    parser.add_argument(
        "--main-dir",
        type=Path,
        default=default_main_dir(root),
        help="Main_MiSTer checkout used for source evidence",
    )
    parser.add_argument(
        "--agent",
        type=Path,
        default=Path(os.environ.get("MAGIK_AGENT", root / "scripts/agent")),
        help="MiSTer MagiK agent entrypoint used for device core listing",
    )
    parser.add_argument(
        "--device-core-list", type=Path, help="fixture TSV: size, mtime, path"
    )
    parser.add_argument(
        "--skip-device", action="store_true", help="do not query the MiSTer"
    )
    parser.add_argument(
        "--output", type=Path, help="write annotated manifest to this path"
    )
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    manifest = read_json(args.manifest)
    if args.device_core_list:
        observed = observed_cores_from_fixture(args.device_core_list)
    elif args.skip_device:
        observed = []
    else:
        observed = observed_cores_from_device(args.agent)
    annotated = annotate_manifest(manifest, args.main_dir, observed)
    text = json.dumps(annotated, indent=2, sort_keys=False) + "\n"
    if args.output:
        args.output.write_text(text, encoding="utf-8")
    else:
        sys.stdout.write(text)
    print(
        "catalog_profile_manifest_tsv\tprofiles={}\tobserved_cores={}\tmain_dir={}".format(
            len(annotated.get("profiles", [])),
            len(observed),
            args.main_dir,
        ),
        file=sys.stderr,
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
