# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

"""Explicit host assurance groups used by CI."""

from __future__ import annotations

import os
import shlex
import subprocess
import time
from pathlib import Path

HOST_SHARDS = {
    "domain": ("core", "rendering"),
    "app": ("core", "ui", "preview", "scenes"),
}

HOST_GROUPS = (
    "static",
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


def _shard(group: str, command: list[str]) -> str:
    if group == "domain":
        return (
            "rendering"
            if "crates/framebuffer-scenes/Cargo.toml" in command
            and "--features" in command
            else "core"
        )
    if command[-1] == "scripts/tests/test-slint-build-contract.py":
        return "scenes"
    if (
        command[:2] in (["cargo", "test"], ["cargo", "clippy"])
        and "--features" in command
    ):
        features = command[command.index("--features") + 1]
        return {
            "ui": "ui",
            "ui-preview": "preview",
            "ui,signed-media-manifests": "preview",
            "ui,bench-scenes": "scenes",
        }.get(features, "core")
    return "core"


def commands(group: str, *, shard: str | None = None) -> list[list[str]]:
    if shard is not None:
        if shard not in HOST_SHARDS.get(group, ()):
            raise ValueError(f"unsupported host assurance shard: {group}/{shard}")
        return [
            command for command in commands(group) if _shard(group, command) == shard
        ]
    if group == "static":
        return []
    if group == "domain":
        manifests = [
            "crates/magik-core/Cargo.toml",
            "crates/framebuffer-scenes/Cargo.toml",
            "crates/particles/Cargo.toml",
            "crates/perf-events/Cargo.toml",
            "crates/screenshot-parade/Cargo.toml",
            "crates/framebuffer-stream/Cargo.toml",
            "crates/media-contract/Cargo.toml",
            "crates/mister-ini/Cargo.toml",
            "mister/platform/runtime/Cargo.toml",
            "mister/platform/contracts/latch/Cargo.toml",
            "mister/platform/contracts/scanout/Cargo.toml",
            "mister/platform/contracts/manifest/Cargo.toml",
        ]
        result = [
            command for manifest in manifests for command in _crate_commands(manifest)
        ]
        for features in (
            "card-axis-filter",
            "card-fast-quantisation",
            "card-axis-filter,card-fast-quantisation",
            "prepared-artwork",
        ):
            result.append(
                [
                    "cargo",
                    "test",
                    "--manifest-path",
                    "crates/framebuffer-scenes/Cargo.toml",
                    "--features",
                    features,
                ]
            )
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
            [
                "python3",
                "scripts/checks/generate-runtime-environment-reference.py",
                "--check",
            ],
            [
                "cargo",
                "check",
                "--locked",
                "--manifest-path",
                manifest,
                "--features",
                "tooling",
            ],
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
                "clippy",
                "--manifest-path",
                manifest,
                "--lib",
                "--tests",
                "--no-default-features",
                "--features",
                "ui",
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
                "ui,bench-scenes",
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
        result = [
            command
            for manifest in (
                "mister/tools/manager/Cargo.toml",
                "tools/usb-video/Cargo.toml",
            )
            for command in _crate_commands(manifest)
        ]
        result.extend(
            [
                [
                    "cargo",
                    "build",
                    "--manifest-path",
                    "mister/tools/manager/Cargo.toml",
                ],
                [
                    "scripts/cargo",
                    "build",
                    "--locked",
                    "--manifest-path",
                    "mister/platform/contracts/manifest/Cargo.toml",
                    "--bin",
                    "platform-manifest-check",
                ],
                ["scripts/tests/test-start-magik.sh"],
            ]
        )
        return result
    raise ValueError(f"unsupported host assurance group: {group}")


def execute(repository: Path, group: str, *, shard: str | None = None) -> None:
    group_commands = commands(group, shard=shard)
    if group == "static":
        from .assurance import execute as execute_fast

        execute_fast(
            repository, ["scripts", "docs", "apps/mister/src", "apps/mister/ui/"]
        )
        return
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
    label = f"{group}/{shard}" if shard else group
    failures: list[subprocess.CalledProcessError] = []
    for index, command in enumerate(group_commands, start=1):
        started = time.monotonic()
        rendered = shlex.join(command)
        print(f"host-assurance[{label}] {index}/{total} start: {rendered}", flush=True)
        outcome = "failed"
        try:
            subprocess.run(command, cwd=repository, env=environment, check=True)
            outcome = "passed"
        except subprocess.CalledProcessError as error:
            failures.append(error)
        finally:
            elapsed = time.monotonic() - started
            print(
                f"host-assurance[{label}] {index}/{total} {outcome} "
                f"elapsed={elapsed:.2f}s: {rendered}",
                flush=True,
            )
    if failures:
        raise failures[0]
