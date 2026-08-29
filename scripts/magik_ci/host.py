# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

"""Explicit host assurance groups used by CI."""

from __future__ import annotations

import os
import shlex
import subprocess
import time
from pathlib import Path

HOST_GROUPS = (
    "static",
    "agent",
    "domain",
    "catalog",
    "app",
    "tools",
)


def _crate_commands(manifest: str) -> list[list[str]]:
    return [
        ["cargo", "fmt", "--manifest-path", manifest, "--check"],
        ["cargo", "test", "--manifest-path", manifest],
        [
            "cargo",
            "clippy",
            "--manifest-path",
            manifest,
            "--all-targets",
            "--",
            "-D",
            "warnings",
        ],
    ]


def commands(group: str) -> list[list[str]]:
    if group == "static":
        return []
    if group == "agent":
        return [
            ["cargo", "fmt", "--manifest-path", "agent-cli/Cargo.toml", "--check"],
            ["cargo", "test", "--manifest-path", "agent-cli/Cargo.toml"],
            [
                "cargo",
                "clippy",
                "--manifest-path",
                "agent-cli/Cargo.toml",
                "--all-targets",
                "--",
                "-D",
                "warnings",
            ],
            [
                "cargo",
                "test",
                "--manifest-path",
                "agent-cli/Cargo.toml",
                "--no-default-features",
                "--features",
                "signed-media-manifests",
            ],
        ]
    if group == "domain":
        manifests = [
            "crates/magik-core/Cargo.toml",
            "crates/framebuffer-scenes/Cargo.toml",
            "crates/particles/Cargo.toml",
            "crates/perf-events/Cargo.toml",
            "crates/screenshot-parade/Cargo.toml",
            "crates/framebuffer-stream/Cargo.toml",
            "crates/agent-protocol/Cargo.toml",
            "crates/media-contract/Cargo.toml",
            "crates/mister-ini/Cargo.toml",
            "mister/platform/runtime/Cargo.toml",
            "mister/platform/contracts/latch/Cargo.toml",
            "mister/platform/contracts/scanout/Cargo.toml",
            "mister/platform/contracts/video-diagnostics/Cargo.toml",
            "mister/platform/contracts/manifest/Cargo.toml",
        ]
        result = [
            command for manifest in manifests for command in _crate_commands(manifest)
        ]
        result.append(
            [
                "cargo",
                "test",
                "--manifest-path",
                "crates/media-contract/Cargo.toml",
                "--no-default-features",
                "--features",
                "signed-media-manifests",
            ]
        )
        return result
    if group == "catalog":
        return [
            ["cargo", "fmt", "--manifest-path", "crates/catalog/Cargo.toml", "--check"],
            [
                "cargo",
                "test",
                "--manifest-path",
                "crates/catalog/Cargo.toml",
                "--features",
                "builder",
            ],
            [
                "cargo",
                "check",
                "--manifest-path",
                "crates/catalog/Cargo.toml",
                "--no-default-features",
            ],
            [
                "cargo",
                "clippy",
                "--manifest-path",
                "crates/catalog/Cargo.toml",
                "--all-features",
                "--all-targets",
                "--",
                "-D",
                "warnings",
            ],
        ]
    if group == "app":
        manifest = "apps/mister/Cargo.toml"
        return [
            ["cargo", "fmt", "--manifest-path", manifest, "--check"],
            [
                "cargo",
                "test",
                "--manifest-path",
                manifest,
                "--lib",
                "--no-default-features",
            ],
            [
                "cargo",
                "clippy",
                "--manifest-path",
                manifest,
                "--lib",
                "--no-default-features",
                "--",
                "-D",
                "warnings",
            ],
            [
                "cargo",
                "test",
                "--manifest-path",
                manifest,
                "--lib",
                "--no-default-features",
                "--features",
                "ui",
                "--",
                "--test-threads=1",
            ],
            [
                "cargo",
                "test",
                "--manifest-path",
                manifest,
                "--lib",
                "--no-default-features",
                "--features",
                "ui",
                "visual_platform::tests::cache_preserving_full_raster_refreshes_moved_deleted_and_rotated_content",
                "--",
                "--ignored",
                "--exact",
            ],
            [
                "cargo",
                "test",
                "--manifest-path",
                manifest,
                "--lib",
                "--no-default-features",
                "--features",
                "ui-preview",
                "--",
                "--test-threads=1",
            ],
            [
                "cargo",
                "test",
                "--manifest-path",
                manifest,
                "--lib",
                "--no-default-features",
                "--features",
                "ui,experiments",
                "--",
                "--test-threads=1",
            ],
            [
                "cargo",
                "test",
                "--manifest-path",
                manifest,
                "--lib",
                "--no-default-features",
                "--features",
                "ui,signed-media-manifests",
                "media_http::tests",
            ],
            ["python3", "scripts/tests/test-slint-build-contract.py"],
        ]
    if group == "tools":
        return [
            command
            for manifest in (
                "mister/tools/agent/Cargo.toml",
                "mister/tools/manager/Cargo.toml",
            )
            for command in _crate_commands(manifest)
        ]
    raise ValueError(f"unsupported host assurance group: {group}")


def execute(repository: Path, group: str) -> None:
    if group == "static":
        from .assurance import execute as execute_fast

        execute_fast(
            repository, ["scripts", "docs", "apps/mister/src", "apps/mister/ui/"]
        )
        return
    group_commands = commands(group)
    total = len(group_commands)
    environment = os.environ.copy()
    environment.update(
        {
            "CARGO_PROFILE_DEV_DEBUG": "0",
            "CARGO_PROFILE_TEST_DEBUG": "0",
        }
    )
    if group == "domain":
        environment["CARGO_TARGET_DIR"] = str(repository / "target/ci-host-domain")
    for index, command in enumerate(group_commands, start=1):
        started = time.monotonic()
        rendered = shlex.join(command)
        print(f"host-assurance[{group}] {index}/{total} start: {rendered}", flush=True)
        outcome = "failed"
        try:
            subprocess.run(command, cwd=repository, env=environment, check=True)
            outcome = "passed"
        finally:
            elapsed = time.monotonic() - started
            print(
                f"host-assurance[{group}] {index}/{total} {outcome} "
                f"elapsed={elapsed:.2f}s: {rendered}",
                flush=True,
            )
