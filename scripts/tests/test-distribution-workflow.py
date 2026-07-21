#!/usr/bin/env python3
# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

"""Static safety contract for the manually published distribution workflow."""

from pathlib import Path
import subprocess

ROOT = Path(__file__).resolve().parents[2]
text = (ROOT / ".github/workflows/distribution.yml").read_text()
cross = (ROOT / "apps/mister/Cross.toml").read_text()
packager = (ROOT / "scripts/package-distribution.sh").read_text()
promotion_guard = ROOT / "scripts/release/check-alpha-promotion.sh"

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
    "- alpha",
    "- beta",
    "- release",
    "fetch-depth: 0",
    "github.ref != 'refs/heads/main'",
    "scripts/agent build runtime-device",
    "environment:\n      name: publish-${{ github.event.inputs.release_channel }}",
    "contents: write",
    "gh release create",
    'if [ "$RELEASE_CHANNEL" = alpha ] || [ "$RELEASE_CHANNEL" = beta ]; then',
    'gh release upload "$RELEASE_CHANNEL" "${assets[@]}" --repo "$GH_REPO" --clobber',
    'repos/$GH_REPO/git/refs/tags/$RELEASE_CHANNEL',
    'repos/$GH_REPO/releases/assets/$asset_id',
    'expected_assets["$(basename "$asset_path")"]',
    'gh release delete "$tag" --repo "$GH_REPO" --cleanup-tag --yes',
    "if: github.event.inputs.release_channel == 'beta'",
    "mister-magik-$RELEASE_CHANNEL-db.json.zip",
    "select-published-release.py platform",
    "mister-magik-platform-v0.*.zip",
    "select-published-release.py game-databases",
    "game-databases-bundle.py verify",
    "game-databases-manifest.json",
    "--checksums build/game-databases/SHA256SUMS",
    "--game-databases-release-dir",
    "platform-bundle-v0.*.json",
    "mister-magik-platform-bundle-v0.2",
    "scripts/release/platform/main-component.py verify",
    "steps.platform.outputs.main_bin",
    "steps.platform.outputs.main_revision",
    "steps.platform.outputs.format == 'v0.1'",
    "MAIN_REF: mister-magik",
    'assert version.encode() in package.read("mister-magik/mister-magik-fb")',
    "initialize_feed_branch()",
    "group: mister-magik-downloader-feed",
    "cancel-in-progress: false",
    'test "$(gh api "repos/$GH_REPO/git/commits/$commit_sha" --jq \'.parents | length\')" = 0',
    'select(. != "mister-magik-alpha-db.json.zip" and . != "mister-magik-beta-db.json.zip" and . != "mister-magik-release-db.json.zip")',
    "scripts/release/check-alpha-promotion.sh",
    'gh api "repos/$GH_REPO/commits/alpha" --jq .sha',
    "assert not alpha_installer.exists()",
)
for value in required:
    assert value in text, f"distribution workflow is missing: {value}"

before_publish, publish = text.split("\n  publish:\n", 1)
assert "contents: write" not in before_publish
assert "permissions:\n      actions: read\n      contents: write" in publish
feed_publish = publish.split("      - name: Update channel feed\n", 1)[1]
publication_verification = publish.split("      - name: Verify publication target\n", 1)[1].split(
    "      - name: Publish GitHub Release\n", 1
)[0]
assert '-f sha="$GITHUB_SHA"' not in feed_publish
assert "tag=alpha" in text
assert "tag=beta" in text
assert "alpha_sha=\"$(git rev-parse -q --verify 'refs/tags/alpha^{commit}' || true)\"" in text
assert "mister-magik-alpha-installer.zip" not in feed_publish
assert 'gh api "repos/$GH_REPO/commits/alpha" --jq .sha' in publication_verification
assert 'if [ -z "$alpha_sha" ]; then' in publication_verification
assert 'if [ "$alpha_sha" != "$GITHUB_SHA" ]; then' in publication_verification
assert "scripts/release/check-alpha-promotion.sh" not in publication_verification

exact_sha = "a" * 40
for channel, alpha_sha, candidate_sha, expected_status in (
    ("alpha", "", exact_sha, 0),
    ("release", "", exact_sha, 0),
    ("beta", "", exact_sha, 1),
    ("beta", "b" * 40, exact_sha, 1),
    ("beta", exact_sha, exact_sha, 0),
):
    result = subprocess.run(
        [promotion_guard, channel, alpha_sha, candidate_sha],
        check=False,
        capture_output=True,
        text=True,
    )
    assert result.returncode == expected_status, (
        channel,
        alpha_sha,
        candidate_sha,
        result.stderr,
    )

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
assert "--main-bin \"$main_bin\"" in text
assert "--main-source-revision \"$main_revision\"" in text

for forbidden in (
    "--mame-sqlite)",
    "--hbmame-sqlite)",
    "--hbmame-sqlite-default)",
    "--game-databases-manifest)",
):
    assert forbidden not in packager, f"production packager still accepts raw database input: {forbidden}"
assert "--game-databases-release-dir)" in packager
assert "mister-magik-platform-bundle-v0.2" in packager
assert "main_sha256=$MAIN_SHA256" in packager

for workflow in (ROOT / ".github/workflows").glob("*.yml"):
    if workflow.name == "game-databases.yml":
        continue
    workflow_text = workflow.read_text()
    assert "game-databases-bundle.py create" not in workflow_text, f"{workflow.name} creates production databases"
    assert "mame-metadata-build" not in workflow_text, f"{workflow.name} builds production database content"

print("distribution workflow contract ok")
