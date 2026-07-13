#!/usr/bin/env python3
"""Static safety contract for the manually published distribution workflow."""

from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
text = (ROOT / ".github/workflows/distribution.yml").read_text()

trigger = text.split("on:\n", 1)[1].split("\npermissions:", 1)[0]
assert trigger.startswith("  workflow_dispatch:\n")
for forbidden in ("\n  push:", "\n  pull_request:", "\n  schedule:", "\n  workflow_call:"):
    assert forbidden not in trigger, f"automatic trigger present: {forbidden.strip()}"

required = (
    "release_channel:",
    "type: choice",
    "- beta",
    "- release",
    "fetch-depth: 0",
    "github.ref != 'refs/heads/main'",
    "magik-gui/build-arm.sh --device --video",
    "environment:\n      name: publish-${{ github.event.inputs.release_channel }}",
    "contents: write",
    "gh release create",
    "mister-magik-$RELEASE_CHANNEL-db.json.zip",
    "platform-component-id.py component fpga",
    "platform-component-id.py component kernel",
    "platform-v0.1-$bundle_id",
    "platform-bundle-v0.1.json",
    "MAIN_REF: mister-magik",
)
for value in required:
    assert value in text, f"distribution workflow is missing: {value}"

before_publish, publish = text.split("\n  publish:\n", 1)
assert "contents: write" not in before_publish
assert "permissions:\n      actions: read\n      contents: write" in publish

for forbidden_input in ("main_ref:", "fpga_run_id:", "scanout_run_id:", "hbmame_ref:"):
    assert forbidden_input not in trigger, f"internal input leaked into dispatch UI: {forbidden_input}"

assert "gh run list --workflow fpga-vblank-latch.yml" not in text
assert "gh run list --workflow kernel-scanout.yml" not in text

print("distribution workflow contract ok")
