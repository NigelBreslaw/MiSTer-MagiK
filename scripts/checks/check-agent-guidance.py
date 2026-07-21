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
        ROOT / "apps/mister/AGENTS.md",
        ROOT / "apps/mister/src/ui_runner/AGENTS.md",
        ROOT / "mister/tools/host/AGENTS.md",
        ROOT / "mister/tools/agent/AGENTS.md",
        ROOT / "scripts/AGENTS.md",
        ROOT / "apps/desktop/AGENTS.md",
        ROOT / "apps/mister/BUILD.md",
        ROOT / "documentation/src/content/docs/contributing/workflow.mdx",
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

    current_guidance = (ROOT / "AGENTS.md", *required, ROOT / "docs/agents/task-map.md")
    forbidden = (
        "scripts/validate",
        "scripts/dev-rust",
        "scripts/doctor",
        "scripts/test-host-tools.sh",
        "scripts/release-check-host.sh",
        "cargo test",
        "cargo check",
        "cargo clippy",
        "cargo fmt",
        "apps/mister/build-arm.sh --check",
    )
    for document in dict.fromkeys(current_guidance):
        text = document.read_text(encoding="utf-8")
        for command in forbidden:
            if command in text:
                raise SystemExit(
                    f"agent guidance bypasses scripts/agent: "
                    f"{document.relative_to(ROOT)} contains {command!r}"
                )

    deployment_forbidden = (
        "scripts/deploy-rust.sh",
        "scripts/deploy-platform.sh",
        "apps/mister/build-arm.sh --device",
        "scripts/mister agent deploy-magik-bin",
    )
    agent_facing_guidance = (
        ROOT / "AGENTS.md",
        ROOT / "docs/agents/README.md",
        ROOT / "docs/agents/task-map.md",
        ROOT / "apps/mister/AGENTS.md",
        ROOT / "scripts/AGENTS.md",
    )
    for document in agent_facing_guidance:
        text = document.read_text(encoding="utf-8")
        for command in deployment_forbidden:
            if command in text:
                raise SystemExit(
                    f"agent deployment guidance bypasses scripts/agent deploy: "
                    f"{document.relative_to(ROOT)} contains {command!r}"
                )

    for caller in (ROOT / "scripts").rglob("*.sh"):
        text = caller.read_text(encoding="utf-8")
        for command in ("deploy-rust.sh", "deploy-platform.sh", "deploy-magik-bin"):
            if command in text:
                raise SystemExit(
                    f"shell deployment orchestration survived CLI takeover: "
                    f"{caller.relative_to(ROOT)} contains {command!r}"
                )

    root_guidance = (ROOT / "AGENTS.md").read_text(encoding="utf-8")
    for command in ("scripts/agent plan", "scripts/agent check", "scripts/agent verify", "scripts/agent deploy"):
        if command not in root_guidance:
            raise SystemExit(f"root agent workflow missing: {command}")

    print("agent guidance checks ok")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
