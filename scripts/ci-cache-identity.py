#!/usr/bin/env python3
# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

"""Generate stable, versioned identities for GitHub Actions build caches."""

from __future__ import annotations

import argparse
import glob
import hashlib
import json
import os
from pathlib import Path
import re


SCHEMA = "v2"
GROUPS: dict[str, tuple[str, ...]] = {
    "rust_abi": (
        "magik-gui/rust-toolchain.toml",
    ),
    "cargo_host": (
        "magik-gui/Cargo.lock",
        "magik-gui/catalog/Cargo.lock",
        "framebuffer-stream/Cargo.lock",
        "tools/magik-agent/Cargo.lock",
        "tools/mister/Cargo.lock",
    ),
    "cargo_arm": (
        "magik-gui/Cargo.lock",
        "framebuffer-stream/Cargo.lock",
    ),
    "cargo_agent": (
        "tools/magik-agent/Cargo.lock",
        "framebuffer-stream/Cargo.lock",
    ),
    "cargo_dist": (
        "magik-gui/Cargo.lock",
        "magik-gui/catalog/Cargo.lock",
        "framebuffer-stream/Cargo.lock",
        "tools/mister/Cargo.lock",
    ),
    "host_target": (
        "magik-gui/rust-toolchain.toml",
        "magik-gui/Cargo.lock",
        "magik-gui/Cargo.toml",
        "magik-gui/catalog/Cargo.lock",
        "magik-gui/catalog/Cargo.toml",
        "framebuffer-stream/Cargo.lock",
        "framebuffer-stream/Cargo.toml",
        "tools/magik-agent/Cargo.lock",
        "tools/magik-agent/Cargo.toml",
        "tools/mister/Cargo.lock",
        "tools/mister/Cargo.toml",
        "magik-gui/build.rs",
        "magik-gui/ui/**/*.slint",
        "magik-gui/ui-generated/**/*",
        "scripts/validate",
        "scripts/dev-rust",
        "scripts/test-host-tools.sh",
    ),
    "arm_target": (
        "magik-gui/rust-toolchain.toml",
        "magik-gui/Cargo.lock",
        "magik-gui/Cargo.toml",
        "framebuffer-stream/Cargo.lock",
        "framebuffer-stream/Cargo.toml",
        "magik-gui/build.rs",
        "magik-gui/build-arm.sh",
        "magik-gui/Cross.toml",
        "magik-gui/Dockerfile.cross-armv7",
        "magik-gui/scripts/build-minimal-ffmpeg.sh",
    ),
    "agent_target": (
        "magik-gui/rust-toolchain.toml",
        "magik-gui/Cross.toml",
        "magik-gui/Dockerfile.cross-armv7",
        "tools/magik-agent/Cargo.lock",
        "tools/magik-agent/Cargo.toml",
        "tools/magik-agent/src/**/*",
        "framebuffer-stream/Cargo.lock",
        "framebuffer-stream/Cargo.toml",
        "framebuffer-stream/src/**/*",
        "scripts/build-mister-agent.sh",
    ),
    "ffmpeg": (
        "magik-gui/Cross.toml",
        "magik-gui/Dockerfile.cross-armv7",
        "magik-gui/scripts/build-minimal-ffmpeg.sh",
    ),
}


def files_for(root: Path, patterns: tuple[str, ...]) -> list[Path]:
    files: set[Path] = set()
    for pattern in patterns:
        for match in glob.glob(str(root / pattern), recursive=True):
            path = Path(match)
            if path.is_file():
                files.add(path)
    return sorted(files, key=lambda path: path.relative_to(root).as_posix())


def digest_group(root: Path, name: str) -> str:
    digest = hashlib.sha256()
    matched = files_for(root, GROUPS[name])
    if not matched:
        raise SystemExit(f"cache identity group {name!r} matched no files")
    for path in matched:
        relative = path.relative_to(root).as_posix().encode()
        digest.update(len(relative).to_bytes(4, "big"))
        digest.update(relative)
        data = path.read_bytes()
        digest.update(len(data).to_bytes(8, "big"))
        digest.update(data)
    return digest.hexdigest()[:20]


def cross_image(root: Path) -> str:
    text = (root / "magik-gui/Cross.toml").read_text(encoding="utf-8")
    match = re.search(r'^image\s*=\s*"([^"]+)"\s*$', text, re.MULTILINE)
    if not match:
        raise SystemExit("magik-gui/Cross.toml has no target image")
    return match.group(1)


def identities(root: Path) -> dict[str, str]:
    values = {"schema": SCHEMA, "cross_image": cross_image(root)}
    values.update({name: digest_group(root, name) for name in GROUPS})
    values["cross_abi"] = hashlib.sha256(
        f"{values['rust_abi']}\0{values['cross_image']}".encode()
    ).hexdigest()[:20]
    return values


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--root", type=Path, default=Path(__file__).resolve().parent.parent)
    parser.add_argument("--github-output", type=Path)
    parser.add_argument("--json", action="store_true")
    parser.add_argument("--value", choices=("cross_image",))
    args = parser.parse_args()

    values = identities(args.root.resolve())
    if args.value:
        print(values[args.value])
        return 0
    if args.github_output:
        with args.github_output.open("a", encoding="utf-8") as output:
            for name, value in values.items():
                output.write(f"{name}={value}\n")
    if args.json or not args.github_output:
        print(json.dumps(values, sort_keys=True))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
