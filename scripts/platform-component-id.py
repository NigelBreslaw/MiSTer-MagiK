#!/usr/bin/env python3
"""Compute stable MiSTer MagiK FPGA/kernel platform component identities."""

from __future__ import annotations

import argparse
import hashlib
import subprocess
import sys
from pathlib import Path

FORMAT = "mister-magik-platform-component-v0.1"
BUNDLE_FORMAT = "mister-magik-platform-bundle-v0.1"

COMPONENT_INPUTS = {
    "fpga": (
        "fpga/menu-vblank-latch",
        "kernel/scanout-slots/mister_magik_scanout_platform.h",
        "scripts/build-fpga-vblank-latch-core.sh",
        "scripts/install-quartus-lite-docker.sh",
        "scripts/check-fpga-quartus-delta.py",
        "scripts/verify-fpga-rbf-manifest.py",
        ".github/workflows/fpga-vblank-latch.yml",
        "scripts/platform-component-id.py",
    ),
    "kernel": (
        "kernel/scanout-slots",
        "scripts/build-scanout-slots-module.sh",
        "scripts/check-scanout-slots-contract.sh",
        "scripts/test-scanout-platform-contract.py",
        ".github/workflows/kernel-scanout.yml",
        "scripts/platform-component-id.py",
    ),
}


class IdentityError(ValueError):
    pass


def run_git(root: Path, *args: str) -> str:
    result = subprocess.run(
        ["git", "-C", str(root), *args], text=True, capture_output=True, check=False
    )
    if result.returncode:
        raise IdentityError(result.stderr.strip() or "git command failed")
    return result.stdout.strip()


def require_clean_repository(root: Path) -> None:
    run_git(root, "rev-parse", "--is-inside-work-tree")
    if run_git(root, "status", "--porcelain", "--untracked-files=all"):
        raise IdentityError("platform component identities require a clean checkout")


def selected_files(root: Path, component: str) -> tuple[Path, ...]:
    files: set[Path] = set()
    for relative in COMPONENT_INPUTS[component]:
        path = root / relative
        if not path.exists():
            raise IdentityError(f"missing {component} input: {relative}")
        if path.is_symlink():
            raise IdentityError(f"symbolic links are not allowed in inputs: {relative}")
        if path.is_dir():
            for child in path.rglob("*"):
                if child.is_symlink():
                    raise IdentityError(f"symbolic links are not allowed in inputs: {child.relative_to(root)}")
                if child.is_file():
                    files.add(child)
        elif path.is_file():
            files.add(path)
        else:
            raise IdentityError(f"unsupported {component} input: {relative}")
    return tuple(sorted(files, key=lambda item: item.relative_to(root).as_posix()))


def component_revision(root: Path, component: str) -> str:
    revision = run_git(root, "log", "-1", "--format=%H", "--", *COMPONENT_INPUTS[component])
    if len(revision) != 40 or any(char not in "0123456789abcdef" for char in revision):
        raise IdentityError(f"no complete history is available for {component} inputs")
    return revision


def component_id(root: Path, component: str) -> tuple[str, str]:
    require_clean_repository(root)
    revision = component_revision(root, component)
    digest = hashlib.sha256()
    digest.update(f"format={FORMAT}\ncomponent={component}\nrevision={revision}\n".encode())
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
    parser.add_argument("--root", type=Path, default=Path(__file__).resolve().parents[1])
    commands = parser.add_subparsers(dest="command", required=True)
    component = commands.add_parser("component")
    component.add_argument("name", choices=sorted(COMPONENT_INPUTS))
    component.add_argument("--github-output", type=Path)
    bundle = commands.add_parser("bundle")
    bundle.add_argument("--fpga-id", required=True)
    bundle.add_argument("--kernel-id", required=True)
    try:
        args = parser.parse_args()
        if args.command == "component":
            identity, revision = component_id(args.root.resolve(), args.name)
            if args.github_output:
                args.github_output.write_text(
                    f"{args.name}_input_sha256={identity}\n{args.name}_component_revision={revision}\n"
                )
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
