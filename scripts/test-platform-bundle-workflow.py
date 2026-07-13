#!/usr/bin/env python3
"""Static contract for the main-only platform bundle promotion workflow."""

from pathlib import Path


text = (Path(__file__).resolve().parents[1] / ".github/workflows/platform-bundle.yml").read_text()

trigger = text.split("on:\n", 1)[1].split("\npermissions:", 1)[0]
assert trigger == "  workflow_dispatch:\n"
for value in (
    "github.ref != 'refs/heads/main'",
    "git branch --show-current)\" = main",
    "platform-component-id.py component fpga",
    "platform-component-id.py component kernel",
    "headBranch == \"main\"",
    "FPGA Vblank Latch RBF",
    "Kernel scanout slots",
    "publish-platform",
    "contents: write",
    "--draft --prerelease",
    "gh release edit",
    "Platform bundle $TAG is already published and valid.",
):
    assert value in text, f"platform bundle workflow is missing: {value}"

print("platform bundle workflow contract ok")
