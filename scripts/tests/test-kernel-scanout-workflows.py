#!/usr/bin/env python3
# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

"""Static contracts for heavyweight and userspace scanout CI."""

from __future__ import annotations

import fnmatch
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
HEAVY = (ROOT / ".github/workflows/kernel-scanout.yml").read_text()
LIGHT = (ROOT / ".github/workflows/scanout-contract.yml").read_text()

HEAVY_PATHS = {
    "mister/platform/kernel/scanout-slots/**",
    "scripts/build-scanout-slots-module.sh",
    "scripts/checks/check-scanout-slots-contract.sh",
    "scripts/tests/test-scanout-platform-contract.py",
    "scripts/platform-component-inputs/kernel-v0.1.txt",
    "scripts/release/platform/platform-component-id.py",
    ".github/workflows/kernel-scanout.yml",
}
LIGHT_PATHS = {
    "mister/platform/runtime/src/framebuffer/scanout_slots.rs",
    "mister/tools/agent/src/scanout_slots_contract.rs",
    "scripts/deploy-platform.sh",
    "scripts/install-slint-boot.sh",
    "documentation/src/content/docs/architecture/kernel-scanout-plugin.mdx",
    ".github/workflows/scanout-contract.yml",
}


def event_paths(text: str, event: str, next_event: str | None) -> set[str]:
    section = text.split(f"  {event}:\n", 1)[1]
    if next_event:
        section = section.split(f"  {next_event}:\n", 1)[0]
    else:
        section = section.split("\npermissions:\n", 1)[0]
    return {
        line.strip()[3:-1]
        for line in section.splitlines()
        if line.strip().startswith('- "') and line.strip().endswith('"')
    }


def triggered(patterns: set[str], changed_path: str) -> bool:
    return any(fnmatch.fnmatchcase(changed_path, pattern) for pattern in patterns)


def step_block(text: str, name: str) -> str:
    block = text.split(f"      - name: {name}\n", 1)[1]
    return block.split("\n      - name:", 1)[0]


assert event_paths(HEAVY, "pull_request", "push") == HEAVY_PATHS
assert event_paths(HEAVY, "push", "workflow_dispatch") == HEAVY_PATHS
assert "  workflow_dispatch:\n" in HEAVY
for required in (
    "contract-and-build:",
    "clang-build:",
    "coccinelle:",
    "Sparse type check",
    "Warning-clean rebuild",
    "Upload attested module",
):
    assert required in HEAVY, f"heavyweight workflow lost required behavior: {required}"
refusal = step_block(HEAVY, "Refuse non-main manual artifact production")
assert "if: github.event_name == 'workflow_dispatch' && github.ref != 'refs/heads/main'" in refusal
upload = step_block(HEAVY, "Upload attested module")
assert "if: github.ref == 'refs/heads/main'" in upload
assert "uses: actions/upload-artifact@v7" in upload

assert event_paths(LIGHT, "pull_request", "push") == LIGHT_PATHS
assert event_paths(LIGHT, "push", None) == LIGHT_PATHS
assert "run: scripts/checks/check-scanout-slots-contract.sh" in LIGHT
for forbidden in (
    "Linux-Kernel_MiSTer",
    "build-scanout-slots-module.sh",
    "coccinelle",
    "upload-artifact",
    "workflow_dispatch",
):
    assert forbidden not in LIGHT, f"lightweight workflow contains heavyweight behavior: {forbidden}"

for unrelated in (
    "mister/tools/agent/src/main.rs",
    "mister/tools/agent/src/sd_browse.rs",
    "docs/benchmarking.md",
    "docs/kernel-scanout-plugin-assurance.md",
    "scripts/scanout-slots-one-shot.sh",
):
    assert not triggered(HEAVY_PATHS, unrelated)
    assert not triggered(LIGHT_PATHS, unrelated)

for userspace_contract in (
    "mister/platform/runtime/src/framebuffer/scanout_slots.rs",
    "mister/tools/agent/src/scanout_slots_contract.rs",
    "scripts/deploy-platform.sh",
    "scripts/install-slint-boot.sh",
):
    assert triggered(LIGHT_PATHS, userspace_contract)
    assert not triggered(HEAVY_PATHS, userspace_contract)

assert triggered(HEAVY_PATHS, "mister/platform/kernel/scanout-slots/mister_magik_scanout_slots.c")
assert not triggered(LIGHT_PATHS, "mister/platform/kernel/scanout-slots/mister_magik_scanout_slots.c")

print("kernel scanout workflow contracts ok")
