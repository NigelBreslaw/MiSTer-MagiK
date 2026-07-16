#!/usr/bin/env python3
# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

"""Check agent routing/authority docs and repository ignore protections."""

from __future__ import annotations

from pathlib import Path
import subprocess


ROOT = Path(__file__).resolve().parents[2]


def main() -> int:
    required = (
        ROOT / "docs/agents/README.md",
        ROOT / "docs/agents/task-map.md",
        ROOT / "docs/agents/file-authority.md",
        ROOT / "magik-gui/AGENTS.md",
        ROOT / "magik-gui/src/ui_runner/AGENTS.md",
        ROOT / "tools/mister/AGENTS.md",
        ROOT / "tools/magik-agent/AGENTS.md",
        ROOT / "scripts/AGENTS.md",
    )
    missing = [str(path.relative_to(ROOT)) for path in required if not path.is_file()]
    if missing:
        raise SystemExit(f"agent guidance missing: {', '.join(missing)}")

    authority = (ROOT / "docs/agents/file-authority.md").read_text(encoding="utf-8")
    for command in (
        "scripts/media/harvest-core-launch-manifest.py",
        "scripts/release/packaging/generate-third-party-licenses.py",
        "documentation/scripts/capture-guide-screenshots.sh",
    ):
        if command not in authority or not (ROOT / command).is_file():
            raise SystemExit(f"file authority regeneration command missing: {command}")

    for path in (
        "build/agent-check.tmp",
        "outputs/agent-check.tmp",
        "target/agent-check.tmp",
        "documentation/node_modules/agent-check.tmp",
        "private/test-fixtures/agent-check.tmp",
        ".env.agent-check",
    ):
        result = subprocess.run(
            ["git", "-C", str(ROOT), "check-ignore", "-q", path],
            check=False,
        )
        if result.returncode != 0:
            raise SystemExit(f"sensitive/generated path is not ignored: {path}")

    print("agent guidance checks ok")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
