#!/usr/bin/env python3
# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

from pathlib import Path

text = (Path(__file__).resolve().parents[2] / ".github/workflows/main-mister.yml").read_text()

required = (
    "workflow_call:",
    "workflow_dispatch:",
    "github.ref != 'refs/heads/main'",
    "Main component creation is restricted to main",
    "Main_MiSTer component",
    "refs/heads/$MAIN_BRANCH",
    "mister-magik-main-v0.1-$component_id",
    "main-artifact-selection.py candidates",
    "main-artifact-selection.py eligible",
    "steps.existing.outputs.hit != 'true'",
    "main_component.py create",
    "main-component.py verify",
    "retention-days: 90",
)
for needle in required:
    assert needle in text, needle

assert "repository: NigelBreslaw/Main_MiSTer" in text
assert "ref: ${{ steps.identity.outputs.main_revision }}" in text
assert "captured_authoritative_head" not in text
assert "path: Main_MiSTer/bin" not in text
assert "main-build-cache" not in text
assert "if: steps.existing.outputs.hit != 'true'\n        working-directory: Main_MiSTer" in text
print("main-mister workflow contract ok")
