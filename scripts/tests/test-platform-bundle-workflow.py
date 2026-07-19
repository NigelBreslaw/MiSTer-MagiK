#!/usr/bin/env python3
# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

"""Static contract for the single manual platform build workflow."""

from pathlib import Path


root = Path(__file__).resolve().parents[2]
workflow = root / ".github/workflows/platform-bundle.yml"
text = workflow.read_text()
trigger = text.split("on:\n", 1)[1].split("\nconcurrency:", 1)[0]

assert "name: Build MiSTer MagiK Platform" in text
assert "workflow_dispatch:" in trigger
assert "publish:" in trigger and "default: false" in trigger
assert "pull_request:" not in trigger and "push:" not in trigger and "workflow_call:" not in trigger
for removed in ("main-mister.yml", "fpga-vblank-latch.yml"):
    assert not (root / ".github/workflows" / removed).exists()

for required in (
    "Plan component reuse and builds",
    "platform-component-id.py component fpga",
    "platform-component-id.py component kernel",
    "main-component.py identity",
    "platform-bundle.py plan-update",
    "main-changed:",
    "fpga-changed:",
    "kernel-changed:",
    "if: needs.plan.outputs.main-changed == 'true'",
    "if: needs.plan.outputs.fpga-changed == 'true'",
    "if: needs.plan.outputs.kernel-changed == 'true'",
    "Reuse unchanged components from latest release",
    "extract-component",
    "reused-from-latest-release",
    "built-in-current-run",
    "platform-main-component",
    "platform-fpga-component",
    "platform-kernel-component",
    "repository: NigelBreslaw/Main_MiSTer",
    "ref: ${{ needs.plan.outputs.main-revision }}",
    "scripts/test-magik-state.sh",
    "scripts/check-magik-patch-surface.sh",
    "main_component.py create",
    "main-component.py verify",
    "platform-bundle-v0.2.json",
    "needs.plan.outputs.update-needed == 'true'",
    "inputs.publish == true",
    "publish-platform",
    "contents: write",
    "--draft --prerelease",
):
    assert required in text, required

assert "recover-platform-component.sh" not in text
assert "gh run download" not in text
assert "actions/artifacts?name=" not in text
assert text.count("  workflow_dispatch:") == 1

bind_step = text.split("      - name: Bind runner temp to build volume\n", 1)[1].split("\n      - name:", 1)[0]
docker_step = text.split("      - name: Prepare Quartus Docker runtime\n", 1)[1].split("\n      - name:", 1)[0]
assert 'sudo mount --bind "$GITHUB_WORKSPACE/.runner-temp" "$RUNNER_TEMP"' in bind_step
assert "GITHUB_ENV" not in bind_step
assert "GITHUB_ENV" in docker_step
print("unified platform workflow contract ok")
