#!/usr/bin/env python3
# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

"""Static safety contract for the manually published distribution workflow."""

from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
text = (ROOT / ".github/workflows/distribution.yml").read_text()
cross = (ROOT / "magik-gui/Cross.toml").read_text()
packager = (ROOT / "scripts/package-distribution.sh").read_text()

for variable in (
    "MISTER_MAGIK_BUILD_NUMBER",
    "MISTER_MAGIK_VERSION",
    "MISTER_MAGIK_BUILD_TIME",
):
    assert f'"{variable}"' in cross, f"cross build does not pass through {variable}"

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
    "magik-gui/build-arm.sh --device",
    "environment:\n      name: publish-${{ github.event.inputs.release_channel }}",
    "contents: write",
    "gh release create",
    "mister-magik-$RELEASE_CHANNEL-db.json.zip",
    "select-published-release.py platform",
    "mister-magik-platform-v0.*.zip",
    "select-published-release.py game-databases",
    "game-databases-bundle.py verify",
    "game-databases-manifest.json",
    "--checksums build/game-databases/SHA256SUMS",
    "--game-databases-release-dir",
    "platform-bundle-v0.1.json",
    "MAIN_REF: mister-magik",
    'assert version.encode() in package.read("mister-magik/mister-magik-fb")',
    "initialize_feed_branch()",
    "group: mister-magik-downloader-feed",
    "cancel-in-progress: false",
    'test "$(gh api "repos/$GH_REPO/git/commits/$commit_sha" --jq \'.parents | length\')" = 0',
    'select(. != "mister-magik-beta-db.json.zip" and . != "mister-magik-release-db.json.zip")',
)
for value in required:
    assert value in text, f"distribution workflow is missing: {value}"

before_publish, publish = text.split("\n  publish:\n", 1)
assert "contents: write" not in before_publish
assert "permissions:\n      actions: read\n      contents: write" in publish
assert '-f sha="$GITHUB_SHA"' not in publish

for forbidden_input in ("main_ref:", "fpga_run_id:", "scanout_run_id:", "hbmame_ref:"):
    assert forbidden_input not in trigger, f"internal input leaked into dispatch UI: {forbidden_input}"

assert "gh run list --workflow fpga-vblank-latch.yml" not in text
assert "gh run list --workflow kernel-scanout.yml" not in text
for forbidden in (
    "platform-component-id.py",
    "repos/Robbbert/hbmame",
    "repos/mamedev/mame",
    "mame-metadata-build",
    "Build HBMAME",
    "MAME_LISTXML_URL",
    "game-databases-bundle.py create",
    "--mame-sqlite",
    "--hbmame-sqlite",
):
    assert forbidden not in text, f"distribution still builds support bundle content: {forbidden}"
assert 'unzip -q -o "$archive" -d build/qualified' in text

for forbidden in (
    "--mame-sqlite)",
    "--hbmame-sqlite)",
    "--hbmame-sqlite-default)",
    "--game-databases-manifest)",
):
    assert forbidden not in packager, f"production packager still accepts raw database input: {forbidden}"
assert "--game-databases-release-dir)" in packager

for workflow in (ROOT / ".github/workflows").glob("*.yml"):
    if workflow.name == "game-databases.yml":
        continue
    workflow_text = workflow.read_text()
    assert "game-databases-bundle.py create" not in workflow_text, f"{workflow.name} creates production databases"
    assert "mame-metadata-build" not in workflow_text, f"{workflow.name} builds production database content"

print("distribution workflow contract ok")
