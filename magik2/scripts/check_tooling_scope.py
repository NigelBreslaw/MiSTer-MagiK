#!/usr/bin/env python3
"""Require an explicit tooling PR marker when changing the 2.0 core."""

from __future__ import annotations

import os
import subprocess
import sys

CONSUMERS = ("magik2/probe/", "magik2/scenarios/")
CORE = (
    "crates/tooling-support/",
    "magik2/",
    "scripts/magik2",
    ".github/workflows/magik2.yml",
    ".github/PULL_REQUEST_TEMPLATE/magik2-tooling.md",
    ".github/CODEOWNERS",
)
# These are the explicit integration seams; unrelated app features remain consumer PRs.
COMPANIONS = (
    "AGENTS.md",
    "scripts/AGENTS.md",
    "scripts/magik_ci/host.py",
    "apps/mister/Cargo.toml",
    "apps/mister/Cargo.lock",
    "apps/mister/src/ui_runner/launcher_loop.rs",
    "apps/mister/src/visual_platform.rs",
    "apps/mister/src/ui_runner/launcher_present/latch.rs",
    "apps/mister/src/ui_runner/launcher_present/orchestrator.rs",
    "agent-cli/src/host/transfer_check.rs",
    "agent-cli/src/host/mod.rs",
    "agent-cli/src/commands/device.rs",
)
# Only the PR first adding this project may include its existing-runtime export
# and repository classification changes. Later tooling PRs stay isolated.
INTRODUCTION_PATHS = {
    "mister/platform/runtime/Cargo.toml",
    "mister/platform/runtime/src/framebuffer/mapped.rs",
    "mister/platform/runtime/src/framebuffer/mod.rs",
    "scripts/checks/repository_policy.py",
    "scripts/tests/test-pre-commit.py",
}


def scope_error(
    paths: list[str], tooling: bool, *, introduction: bool = False
) -> str | None:
    core = [
        path
        for path in paths
        if path.startswith(CORE) and not path.startswith(CONSUMERS)
    ]
    if not core:
        return None
    if not tooling:
        return "MagiK 2 core changes require the magik2-tooling PR label and tooling review."
    unrelated = [
        path
        for path in paths
        if not path.startswith(CORE)
        and path not in COMPANIONS
        and not (introduction and path in INTRODUCTION_PATHS)
    ]
    if unrelated:
        return "Keep the tooling PR focused; unrelated changes: " + ", ".join(unrelated)
    return None


def changed_paths(base: str, head: str) -> list[str]:
    output = subprocess.check_output(
        ["git", "diff", "--name-only", f"{base}...{head}"], text=True
    )
    return output.splitlines()


def main() -> int:
    base = os.environ.get("BASE_SHA")
    head = os.environ.get("HEAD_SHA", "HEAD")
    if not base:
        print("BASE_SHA is required for the tooling scope check", file=sys.stderr)
        return 2
    added = subprocess.check_output(
        [
            "git",
            "diff",
            "--name-only",
            "--diff-filter=A",
            f"{base}...{head}",
            "--",
            "magik2/AGENTS.md",
        ],
        text=True,
    ).splitlines()
    error = scope_error(
        changed_paths(base, head),
        os.environ.get("MAGIK2_TOOLING_PR") == "1",
        introduction="magik2/AGENTS.md" in added,
    )
    if error:
        print(error, file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
