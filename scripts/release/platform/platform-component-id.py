#!/usr/bin/env python3
# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

"""Compute stable MiSTer MagiK FPGA/kernel platform component identities."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import re
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
    digest.update(f"format={FORMAT}\ncomponent={component}\n".encode())
    for path in selected_files(root, component):
        relative = path.relative_to(root).as_posix()
        file_hash = hashlib.sha256(
            identity_bytes(component, relative, path.read_bytes())
        ).hexdigest()
        digest.update(f"path={relative}\nsha256={file_hash}\n".encode())
    return digest.hexdigest(), revision


def identity_bytes(component: str, relative: str, data: bytes) -> bytes:
    """Build jobs are inputs; unrelated planning/publication jobs are not."""
    if relative != ".github/workflows/platform-bundle.yml":
        return data
    job = {"fpga": "build-fpga", "kernel": "build-kernel"}[component]
    matches = re.findall(
        rf"^  {job}:\n.*?(?=^  [\w-]+:|\Z)",
        data.decode(),
        re.MULTILINE | re.DOTALL,
    )
    if len(matches) != 1:
        raise IdentityError(f"expected one {job} job in {relative}")
    # Global defaults or non-Main environment settings can affect either build.
    # The three existing Main-only settings must not invalidate FPGA/kernel.
    globals_ = []
    for key in ("env", "defaults"):
        for block in re.findall(
            rf"^{key}:\n.*?(?=^[\w-]+:|\Z)", data.decode(), re.MULTILINE | re.DOTALL
        ):
            lines = [
                line
                for line in block.splitlines()
                if not re.match(r"^  MAIN_(REPOSITORY|BRANCH|TOOLCHAIN_VERSION):", line)
            ]
            globals_.append("\n".join(lines).rstrip())
    return "\n".join([*globals_, matches[0].rstrip()]).encode()


def equivalent_inputs(root: Path, component: str, revision: str) -> bool:
    """Compare actual input blobs, even across reconstructed Git history.

    Use the current input selection on both trees. Identity implementation and
    selection-file bytes describe the hashing scheme, not the built artifact.
    A release keeps its original identity and provenance when inputs match.
    """
    if not re.fullmatch(r"[0-9a-f]{40}", revision):
        return False
    inputs = component_inputs(root, component)[:-2]
    try:
        previous = run_git(
            root, "ls-tree", "-r", "--full-tree", revision, "--", *inputs
        )
        current = run_git(root, "ls-tree", "-r", "--full-tree", "HEAD", "--", *inputs)

        def entries(tree: str) -> dict[str, tuple[str, str]]:
            result = {}
            for line in tree.splitlines():
                metadata, name = line.split("\t", 1)
                mode, kind, oid = metadata.split()
                if kind != "blob" or mode not in {"100644", "100755"}:
                    raise IdentityError("component inputs must be regular files")
                result[name] = (mode, oid)
            return result

        old, new = entries(previous), entries(current)
        if not old or old.keys() != new.keys():
            return False
        for name, (mode, oid) in new.items():
            old_mode, old_oid = old[name]
            if mode != old_mode:
                return False
            if oid != old_oid:
                if name != ".github/workflows/platform-bundle.yml":
                    return False
                before = run_git(root, "cat-file", "blob", old_oid).encode()
                after = run_git(root, "cat-file", "blob", oid).encode()
                # Only the workflow has a deliberately selected input region.
                if identity_bytes(component, name, before) != identity_bytes(
                    component, name, after
                ):
                    return False
        return True
    except IdentityError:
        return False


def release_identities(root: Path, manifest: dict) -> dict[str, str]:
    require_clean_repository(root)
    if manifest.get("format") != "mister-magik-platform-bundle-v0.2":
        raise IdentityError("unsupported release manifest")
    result = {}
    for component in ("fpga", "kernel"):
        desired, _ = component_id(root, component)
        origin = manifest.get("components", {}).get(component, {})
        previous = manifest.get(f"{component}_input_sha256", "")
        reuse = (
            origin.get("component") == component
            and origin.get("head_branch") == "main"
            and re.fullmatch(r"[0-9a-f]{64}", previous)
            and equivalent_inputs(root, component, origin.get("head_sha", ""))
        )
        result[f"{component}_id"] = previous if reuse else desired
        result[f"{component}_release_equivalent"] = "true" if reuse else "false"
    return result


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
    release = commands.add_parser("release-identities")
    release.add_argument("--manifest", type=Path, required=True)
    release.add_argument("--github-output", type=Path, required=True)
    equivalent = commands.add_parser("equivalent")
    equivalent.add_argument("name", choices=sorted(COMPONENT_INPUT_MANIFESTS))
    equivalent.add_argument("--revision", required=True)
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
        if args.command == "release-identities":
            values = release_identities(
                args.root.resolve(), json.loads(args.manifest.read_text())
            )
            with args.github_output.open("a") as output:
                for key, value in values.items():
                    output.write(f"{key}={value}\n")
            print(json.dumps(values, sort_keys=True))
        elif args.command == "equivalent":
            require_clean_repository(args.root.resolve())
            return (
                0
                if equivalent_inputs(args.root.resolve(), args.name, args.revision)
                else 1
            )
        elif args.command == "component":
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
