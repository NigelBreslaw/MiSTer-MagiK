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
        "apps/mister/rust-toolchain.toml",
    ),
    "cargo_host": (
        "apps/mister/Cargo.lock",
        "crates/magik-core/Cargo.lock",
        "crates/catalog/Cargo.lock",
        "apps/desktop/Cargo.lock",
        "crates/framebuffer-stream/Cargo.lock",
        "mister/tools/agent/Cargo.lock",
        "agent-cli/Cargo.lock",
        "mister/platform/runtime/Cargo.lock",
    ),
    "cargo_arm": (
        "apps/mister/Cargo.lock",
        "crates/framebuffer-stream/Cargo.lock",
        "mister/platform/runtime/Cargo.lock",
    ),
    "cargo_agent": (
        "mister/tools/agent/Cargo.lock",
        "crates/framebuffer-stream/Cargo.lock",
    ),
    "cargo_dist": (
        "apps/mister/Cargo.lock",
        "crates/magik-core/Cargo.lock",
        "crates/catalog/Cargo.lock",
        "crates/framebuffer-stream/Cargo.lock",
        "agent-cli/Cargo.lock",
        "mister/platform/runtime/Cargo.lock",
    ),
    "agent_cli": (
        "apps/mister/rust-toolchain.toml",
        "scripts/agent",
        "agent-cli/Cargo.lock",
        "agent-cli/Cargo.toml",
        "agent-cli/src/**/*.rs",
        "crates/catalog/Cargo.toml",
        "crates/catalog/data/**/*.json",
        "crates/catalog/src/**/*.rs",
        "crates/media-contract/Cargo.toml",
        "crates/media-contract/src/**/*.rs",
        "crates/agent-protocol/Cargo.toml",
        "crates/agent-protocol/src/**/*.rs",
    ),
    "agent_cli_deps": (
        "apps/mister/rust-toolchain.toml",
        "agent-cli/Cargo.lock",
        "agent-cli/Cargo.toml",
        "crates/catalog/Cargo.toml",
        "crates/media-contract/Cargo.toml",
        "crates/agent-protocol/Cargo.toml",
    ),
    "host_target": (
        "apps/mister/rust-toolchain.toml",
        "apps/mister/Cargo.lock",
        "apps/mister/Cargo.toml",
        "apps/mister/src/**/*.rs",
        "crates/magik-core/Cargo.toml",
        "crates/magik-core/src/**/*.rs",
        "crates/catalog/Cargo.lock",
        "crates/catalog/Cargo.toml",
        "crates/catalog/data/**/*.json",
        "crates/catalog/src/**/*.rs",
        "crates/catalog/tests/**/*.rs",
        "apps/desktop/Cargo.lock",
        "apps/desktop/Cargo.toml",
        "apps/desktop/build.rs",
        "apps/desktop/src/**/*.rs",
        "apps/desktop/ui/**/*.slint",
        "apps/desktop/vendor/github-app/packages/primer-slint/**/*.slint",
        "apps/desktop/vendor/github-app/packages/primer-slint/**/*.svg",
        "apps/desktop/vendor/github-app/packages/primer-slint/**/*.ttf",
        "crates/framebuffer-stream/Cargo.lock",
        "crates/framebuffer-stream/Cargo.toml",
        "crates/framebuffer-stream/src/**/*.rs",
        "mister/tools/agent/Cargo.lock",
        "mister/tools/agent/Cargo.toml",
        "mister/tools/agent/src/**/*.rs",
        "agent-cli/Cargo.lock",
        "mister/platform/runtime/Cargo.toml",
        "mister/platform/runtime/build.rs",
        "mister/platform/runtime/src/**/*.rs",
        "apps/mister/build.rs",
        "apps/mister/ui/**/*.slint",
        "apps/mister/ui/**/*.ttf",
        "apps/mister/ui/**/*.svg",
        "apps/mister/ui-generated/Cargo.toml",
        "apps/mister/ui-generated/build.rs",
        "apps/mister/ui-generated/src/**/*.rs",
        "scripts/agent",
        "agent-cli/Cargo.toml",
        "agent-cli/src/**/*.rs",
    ),
    "arm_target": (
        "apps/mister/rust-toolchain.toml",
        "apps/mister/Cargo.lock",
        "apps/mister/Cargo.toml",
        "apps/mister/src/**/*.rs",
        "crates/magik-core/Cargo.toml",
        "crates/magik-core/src/**/*.rs",
        "crates/catalog/Cargo.lock",
        "crates/catalog/Cargo.toml",
        "crates/catalog/data/**/*.json",
        "crates/catalog/src/**/*.rs",
        "crates/framebuffer-stream/Cargo.lock",
        "crates/framebuffer-stream/Cargo.toml",
        "crates/framebuffer-stream/src/**/*.rs",
        "mister/platform/runtime/Cargo.toml",
        "mister/platform/runtime/build.rs",
        "mister/platform/runtime/src/**/*.rs",
        "apps/mister/build.rs",
        "apps/mister/ui/**/*.slint",
        "apps/mister/ui/**/*.ttf",
        "apps/mister/ui/**/*.svg",
        "apps/mister/ui-generated/Cargo.toml",
        "apps/mister/ui-generated/build.rs",
        "apps/mister/ui-generated/src/**/*.rs",
        "agent-cli/src/build.rs",
        "apps/mister/Cross.toml",
        "apps/mister/Dockerfile.cross-armv7",
    ),
    "agent_target": (
        "apps/mister/rust-toolchain.toml",
        "apps/mister/Cross.toml",
        "apps/mister/Dockerfile.cross-armv7",
        "mister/tools/agent/Cargo.lock",
        "mister/tools/agent/Cargo.toml",
        "mister/tools/agent/src/**/*.rs",
        "crates/framebuffer-stream/Cargo.lock",
        "crates/framebuffer-stream/Cargo.toml",
        "crates/framebuffer-stream/src/**/*.rs",
        "agent-cli/src/build.rs",
    ),
    "ffmpeg": (
        "apps/mister/Cross.toml",
        "apps/mister/Dockerfile.cross-armv7",
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
    text = (root / "apps/mister/Cross.toml").read_text(encoding="utf-8")
    match = re.search(r'^image\s*=\s*"([^"]+)"\s*$', text, re.MULTILINE)
    if not match:
        raise SystemExit("apps/mister/Cross.toml has no target image")
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
    parser.add_argument("--root", type=Path, default=Path(__file__).resolve().parents[2])
    parser.add_argument("--github-output", type=Path)
    parser.add_argument("--json", action="store_true")
    parser.add_argument("--value", choices=("cross_image", *GROUPS))
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
