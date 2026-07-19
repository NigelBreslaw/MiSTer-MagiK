#!/usr/bin/env python3
# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

"""Static contract for the main-only platform bundle promotion workflow."""

from pathlib import Path


text = (Path(__file__).resolve().parents[2] / ".github/workflows/platform-bundle.yml").read_text()

trigger = text.split("on:\n", 1)[1].split("\nconcurrency:", 1)[0]
assert "workflow_dispatch:" in trigger
assert "publish:" in trigger
assert "default: false" in trigger
for value in (
    "git ls-remote https://github.com/NigelBreslaw/Main_MiSTer.git refs/heads/mister-magik",
    "uses: ./.github/workflows/main-mister.yml",
    "main_revision: ${{ needs.resolve-main.outputs.revision }}",
    "github.ref != 'refs/heads/main'",
    "git branch --show-current)\" = main",
    "platform-component-id.py component fpga",
    "platform-component-id.py component kernel",
    "headBranch == \"main\"",
    "FPGA Vblank Latch RBF",
    "Kernel scanout slots",
    "select-published-release.py platform --field version",
    "platform-bundle.py plan-update",
    "--main-id \"$MAIN_ID\"",
    "actions/download-artifact@v8",
    "--main-dir build/platform-input/main",
    "--main-run-id \"$MAIN_RUN_ID\"",
    "--main-head-sha \"$MAIN_REVISION\"",
    "platform-bundle-v0.2.json",
    "steps.plan.outputs.update_needed == 'true'",
    "--release-version '${{ steps.plan.outputs.next_version }}'",
    "platform-bundle-v0.${{ needs.assemble.outputs.next-version }}-candidate",
    "numbered platform releases are immutable",
    "publish-platform",
    "contents: write",
    "--draft --prerelease",
    "gh release edit",
):
    assert value in text, f"platform bundle workflow is missing: {value}"

publish = text.split("  publish:\n", 1)[1]
assert "- uses: actions/checkout@v7" in publish
assert "inputs.publish == true" in publish

print("platform bundle workflow contract ok")
