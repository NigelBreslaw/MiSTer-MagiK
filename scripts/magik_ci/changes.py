# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

"""Select optional CI checks from the complete PR or push diff."""

from __future__ import annotations

import json
import re
from pathlib import Path
from typing import Any

from .common import github_output, run

PATHS: dict[str, tuple[str, ...]] = {
    "tooling": (
        ".github/workflows/ci.yml",
        "scripts/magik_ci/changes.py",
        "scripts/tests/test_ci_changes.py",
        "crates/catalog/**",
        "crates/tooling-support/**",
        "apps/mister/**",
        "magik/**",
        "scripts/magik",
        "crates/framebuffer-stream/**",
        "crates/magik-core/**",
        "mister/platform/runtime/**",
        "mister/platform/contracts/latch/**",
        "mister/platform/contracts/scanout/**",
        "apps/mister/ui/fonts/Jersey25-Regular.ttf",
    ),
    "scanout": (
        ".github/workflows/ci.yml",
        "scripts/magik_ci/changes.py",
        "scripts/tests/test_ci_changes.py",
        "mister/platform/runtime/src/framebuffer/hidden_scanout.rs",
        "mister/platform/runtime/src/framebuffer/scanout_slots.rs",
        "mister/platform/contracts/scanout/src/lib.rs",
        "magik/agent/src/publication.rs",
        "mister/platform/contracts/platform-v3.schema.toml",
        "scripts/checks/check-scanout-slots-contract.sh",
        "documentation/src/content/docs/architecture/kernel-scanout-plugin.mdx",
    ),
    "fpga": (
        ".github/workflows/ci.yml",
        "scripts/magik_ci/changes.py",
        "scripts/tests/test_ci_changes.py",
        "mister/platform/fpga/menu-vblank-latch/**",
        "mister/platform/contracts/latch/**",
        "scripts/*fpga*latch*",
        "scripts/build-fpga-vblank-latch-core.sh",
        "scripts/prepare-fpga-menu-signoff.py",
        "scripts/checks/check-latch-protocol.py",
        "scripts/checks/check-fpga-latch-coverage.py",
        "scripts/checks/check-fpga-latch-integration.py",
        "scripts/checks/check-fpga-quartus-delta.py",
        "scripts/checks/generate-latch-protocol.py",
        "scripts/checks/generate-video-diagnostics-protocol.py",
        "scripts/checks/generate-hdmi-evidence-protocol.py",
        "scripts/checks/verify-crt-qualification-evidence.py",
        "scripts/checks/verify-fpga-rbf-manifest.py",
        "scripts/platform-component-inputs/fpga-v0.1.txt",
        "scripts/platform-component-inputs/fpga-synthesis-v0.1.txt",
        "scripts/release/platform/platform-component-id.py",
        "scripts/tests/test-crt-qualification-evidence.py",
        "scripts/tests/test-fpga-rbf-manifest.py",
        "scripts/tests/test-fpga-quartus-delta.py",
        "scripts/tests/test-fpga-vblank-latch.sh",
        "scripts/tests/test-platform-component-id.py",
        ".github/workflows/platform-bundle.yml",
    ),
    "kernel": (
        ".github/workflows/ci.yml",
        "scripts/magik_ci/changes.py",
        "scripts/tests/test_ci_changes.py",
        "mister/platform/kernel/scanout-slots/**",
        "scripts/build-scanout-slots-module.sh",
        "scripts/checks/check-scanout-slots-contract.sh",
        "scripts/tests/test-scanout-platform-contract.py",
        "scripts/platform-component-inputs/kernel-v0.1.txt",
        "scripts/release/platform/platform-component-id.py",
        ".github/workflows/platform-bundle.yml",
    ),
}


def select(repository: Path, event_name: str, event: dict[str, Any]) -> dict[str, bool]:
    if event_name == "workflow_dispatch":
        return dict.fromkeys(PATHS, True)
    if event_name == "pull_request":
        base = event["pull_request"]["base"]["sha"]
        head = event["pull_request"]["head"]["sha"]
        separator = "..."
    elif event_name == "push":
        base, head = event["before"], event["after"]
        separator = ".."
    else:
        raise ValueError(f"unsupported CI event: {event_name}")
    if not all(re.fullmatch(r"[0-9a-f]{40}", sha) for sha in (base, head)):
        raise ValueError("CI diff requires full commit SHAs")
    if base == "0" * 40:
        return dict.fromkeys(PATHS, True)
    return {
        group: bool(
            run(
                [
                    "git",
                    "diff",
                    "--name-only",
                    "--no-renames",
                    "-z",
                    f"{base}{separator}{head}",
                    "--",
                    *(f":(glob){path}" for path in paths),
                ],
                cwd=repository,
            ).stdout
        )
        for group, paths in PATHS.items()
    }


def execute(repository: Path, event_name: str, event_path: Path, output: Path) -> None:
    values = select(repository, event_name, json.loads(event_path.read_text()))
    github_output(output, {key: value for key, value in values.items()})
    print(json.dumps(values, sort_keys=True))
