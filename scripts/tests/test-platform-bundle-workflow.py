#!/usr/bin/env python3
# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

"""Static contract for the single manual platform build workflow."""

from pathlib import Path


root = Path(__file__).resolve().parents[2]
workflow = root / ".github/workflows/platform-bundle.yml"
text = workflow.read_text()
fast_text = (root / ".github/workflows/fpga-latch-fast.yml").read_text()
rtl_test = (root / "scripts/tests/test-fpga-vblank-latch.sh").read_text()
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
    "platform-main-v0.1-${{ needs.plan.outputs.main-id }}",
    "platform-fpga-v0.1-${{ needs.plan.outputs.fpga-id }}",
    "platform-kernel-v0.1-${{ needs.plan.outputs.kernel-id }}",
    "platform-artifact-selection.py candidates",
    "platform-artifact-selection.py eligible-run",
    "verify-component",
    "write-component-cache",
    ".origin.run_id",
    ".origin.head_sha",
    "candidate-hit:",
    "main-cache-hit:",
    "fpga-cache-hit:",
    "kernel-cache-hit:",
    "reused-from-actions-cache",
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
assert "gh run download" in text
assert "actions/artifacts?per_page=100" in text
assert text.count("  workflow_dispatch:") == 1
assert text.count("retention-days: 30") == 8
for simulation in (
    "tb_mister_magik_vblank_latch",
    "tb_mister_magik_crt_timing",
    "tb_mister_magik_crt_reader",
):
    assert simulation in rtl_test, simulation
assert "scripts/tests/test-fpga-vblank-latch.sh" in fast_text
assert "check-fpga-latch-coverage.py" in fast_text
assert "check-fpga-quartus-delta.py" in text
assert "quartus-delta-signoff.tsv" in text
assert "latch_protocol_version" in (root / "scripts/build-fpga-vblank-latch-core.sh").read_text()

for component in ("main", "fpga", "kernel"):
    job = text.split(f"  build-{component}:\n", 1)[1].split("\n  build-", 1)[0] if component != "kernel" else text.split("  build-kernel:\n", 1)[1].split("\n  build-fpga:\n", 1)[0]
    assert f"needs.plan.outputs.{component}-cache-hit != 'true'" in job
    assert "needs.plan.outputs.candidate-hit != 'true'" in job

assemble = text.split("  assemble:\n", 1)[1].split("\n  publish:\n", 1)[0]
assert "needs.plan.outputs.candidate-hit != 'true'" in assemble
publish = text.split("  publish:\n", 1)[1]
assert "needs.plan.outputs.candidate-hit == 'true'" in publish

bind_step = text.split("      - name: Bind runner temp to build volume\n", 1)[1].split("\n      - name:", 1)[0]
docker_step = text.split("      - name: Prepare Quartus Docker runtime\n", 1)[1].split("\n      - name:", 1)[0]
assert 'sudo mount --bind "$GITHUB_WORKSPACE/.runner-temp" "$RUNNER_TEMP"' in bind_step
assert "GITHUB_ENV" not in bind_step
assert "GITHUB_ENV" in docker_step
restore_step = text.split("      - name: Restore private Quartus runtime\n", 1)[1].split("\n      - name:", 1)[0]
save_step = text.split("      - name: Save private Quartus runtime\n", 1)[1].split("\n      - uses:", 1)[0]
assert "hit=degraded" in restore_step
assert "installing the pinned runtime without updating the cache" in restore_step
assert "steps.quartus-cache.outputs.hit == 'false'" in save_step
print("unified platform workflow contract ok")
