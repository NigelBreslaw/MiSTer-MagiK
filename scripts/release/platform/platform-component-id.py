#!/usr/bin/env python3
# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

"""Compute stable MiSTer MagiK FPGA/kernel platform component identities."""

from __future__ import annotations

import argparse
import hashlib
import os
import subprocess
import sys
from pathlib import Path

FORMAT = "mister-magik-platform-component-v0.1"
BUNDLE_FORMAT = "mister-magik-platform-bundle-v0.1"

COMPONENT_INPUT_MANIFESTS = {
    "fpga": "scripts/platform-component-inputs/fpga-v0.1.txt",
    "fpga-synthesis": "scripts/platform-component-inputs/fpga-synthesis-v0.1.txt",
    "kernel": "scripts/platform-component-inputs/kernel-v0.1.txt",
}
IDENTITY_IMPLEMENTATION = "scripts/release/platform/platform-component-id.py"


class IdentityError(ValueError):
    pass


def run_git(root: Path, *args: str) -> str:
    env = os.environ.copy()
    for name in (
        "GIT_DIR",
        "GIT_WORK_TREE",
        "GIT_INDEX_FILE",
        "GIT_OBJECT_DIRECTORY",
        "GIT_ALTERNATE_OBJECT_DIRECTORIES",
        "GIT_COMMON_DIR",
    ):
        env.pop(name, None)
    result = subprocess.run(
        ["git", "-C", str(root), *args],
        text=True,
        capture_output=True,
        check=False,
        env=env,
    )
    if result.returncode:
        raise IdentityError(result.stderr.strip() or "git command failed")
    return result.stdout.strip()


def require_clean_repository(root: Path) -> None:
    run_git(root, "rev-parse", "--is-inside-work-tree")
    if run_git(root, "status", "--porcelain", "--untracked-files=no"):
        raise IdentityError("platform component identities require a clean checkout")


def component_inputs(root: Path, component: str) -> tuple[str, ...]:
    manifest_relative = COMPONENT_INPUT_MANIFESTS[component]
    manifest = root / manifest_relative
    if not manifest.is_file() or manifest.is_symlink():
        raise IdentityError(
            f"missing or invalid {component} input manifest: {manifest_relative}"
        )
    inputs: list[str] = []
    seen: set[str] = set()
    for line_number, raw_line in enumerate(manifest.read_text().splitlines(), 1):
        relative = raw_line.strip()
        if not relative or relative.startswith("#"):
            continue
        path = Path(relative)
        if path.is_absolute() or ".." in path.parts or path.as_posix() != relative:
            raise IdentityError(
                f"invalid {component} input at {manifest_relative}:{line_number}: {relative}"
            )
        if relative in seen:
            raise IdentityError(
                f"duplicate {component} input at {manifest_relative}:{line_number}: {relative}"
            )
        seen.add(relative)
        inputs.append(relative)
    if not inputs:
        raise IdentityError(f"empty {component} input manifest: {manifest_relative}")
    return (*inputs, manifest_relative, IDENTITY_IMPLEMENTATION)


def selected_files(root: Path, component: str) -> tuple[Path, ...]:
    inputs = component_inputs(root, component)
    tracked = run_git(root, "ls-files", "-z", "--", *inputs)
    files: set[Path] = set()
    for relative in filter(None, tracked.split("\0")):
        path = root / relative
        if not path.is_file():
            raise IdentityError(
                f"tracked {component} input is not a regular file: {relative}"
            )
        files.add(path)
    for relative in inputs:
        path = root / relative
        if not path.exists():
            raise IdentityError(f"missing {component} input: {relative}")
        if path.is_symlink():
            raise IdentityError(f"symbolic links are not allowed in inputs: {relative}")
        if path.is_dir():
            if not any(path in file.parents for file in files):
                raise IdentityError(f"no tracked {component} inputs under: {relative}")
        elif path.is_file():
            if path not in files:
                raise IdentityError(f"input is not tracked: {relative}")
        else:
            raise IdentityError(f"unsupported {component} input: {relative}")
    return tuple(sorted(files, key=lambda item: item.relative_to(root).as_posix()))


def component_revision(root: Path, component: str) -> str:
    revision = run_git(
        root, "log", "-1", "--format=%H", "--", *component_inputs(root, component)
    )
    if len(revision) != 40 or any(char not in "0123456789abcdef" for char in revision):
        raise IdentityError(f"no complete history is available for {component} inputs")
    return revision


def component_id(root: Path, component: str) -> tuple[str, str]:
    require_clean_repository(root)
    revision = component_revision(root, component)
    digest = hashlib.sha256()
    digest.update(
        f"format={FORMAT}\ncomponent={component}\nrevision={revision}\n".encode()
    )
    for path in selected_files(root, component):
        relative = path.relative_to(root).as_posix()
        file_hash = hashlib.sha256(path.read_bytes()).hexdigest()
        digest.update(f"path={relative}\nsha256={file_hash}\n".encode())
    return digest.hexdigest(), revision


def bundle_id(fpga_id: str, kernel_id: str) -> str:
    for name, value in (("fpga", fpga_id), ("kernel", kernel_id)):
        if len(value) != 64 or any(char not in "0123456789abcdef" for char in value):
            raise IdentityError(f"invalid {name} component identity")
    return hashlib.sha256(
        f"format={BUNDLE_FORMAT}\nfpga_input_sha256={fpga_id}\nkernel_input_sha256={kernel_id}\n".encode()
    ).hexdigest()


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--root", type=Path, default=Path(__file__).resolve().parents[3]
    )
    commands = parser.add_subparsers(dest="command", required=True)
    component = commands.add_parser("component")
    component.add_argument("name", choices=sorted(COMPONENT_INPUT_MANIFESTS))
    component_output = component.add_mutually_exclusive_group()
    component_output.add_argument("--github-output", type=Path)
    component_output.add_argument(
        "--revision-only",
        action="store_true",
        help="print the canonical last-changing input revision instead of the identity",
    )
    bundle = commands.add_parser("bundle")
    bundle.add_argument("--fpga-id", required=True)
    bundle.add_argument("--kernel-id", required=True)
    try:
        args = parser.parse_args()
        if args.command == "component":
            identity, revision = component_id(args.root.resolve(), args.name)
            if args.revision_only:
                print(revision)
            elif args.github_output:
                with args.github_output.open("a") as output:
                    output.write(f"{args.name}_input_sha256={identity}\n")
                    output.write(f"{args.name}_component_revision={revision}\n")
            else:
                print(identity)
        else:
            print(bundle_id(args.fpga_id, args.kernel_id))
    except IdentityError as error:
        print(f"platform component identity error: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
