#!/usr/bin/env python3
# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

"""Static contract for the main-only platform bundle promotion workflow."""

from pathlib import Path


text = (Path(__file__).resolve().parents[2] / ".github/workflows/platform-bundle.yml").read_text()

trigger = text.split("on:\n", 1)[1].split("\nconcurrency:", 1)[0]
assert trigger == "  workflow_dispatch:\n"
for value in (
    "github.ref != 'refs/heads/main'",
    "git branch --show-current)\" = main",
    "platform-component-id.py component fpga",
    "platform-component-id.py component kernel",
    "headBranch == \"main\"",
    "FPGA Vblank Latch RBF",
    "Kernel scanout slots",
    "select-published-release.py platform --field version",
    "platform-bundle.py plan-update",
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
assert "if: needs.assemble.outputs.update-needed == 'true'" in publish

print("platform bundle workflow contract ok")
