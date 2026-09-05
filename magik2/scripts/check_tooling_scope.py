#!/usr/bin/env python3
"""Require an explicit tooling PR marker when changing the 2.0 core."""

from __future__ import annotations

import os
import subprocess
import sys

CONSUMERS = ("magik2/probe/", "magik2/scenarios/")
CORE = ("magik2/", "scripts/magik2", ".github/workflows/magik2.yml", ".github/PULL_REQUEST_TEMPLATE/magik2-tooling.md", ".github/CODEOWNERS")
COMPANIONS = ("AGENTS.md", "scripts/AGENTS.md")


def scope_error(paths: list[str], tooling: bool) -> str | None:
    core = [path for path in paths if path.startswith(CORE) and not path.startswith(CONSUMERS)]
    if not core:
        return None
    if not tooling:
        return "MagiK 2 core changes require the magik2-tooling PR label and tooling review."
    unrelated = [path for path in paths if not path.startswith(CORE) and path not in COMPANIONS]
    if unrelated:
        return "Keep the tooling PR focused; unrelated changes: " + ", ".join(unrelated)
    return None


def changed_paths(base: str, head: str) -> list[str]:
    output = subprocess.check_output(["git", "diff", "--name-only", f"{base}...{head}"], text=True)
    return output.splitlines()


def main() -> int:
    base = os.environ.get("BASE_SHA")
    head = os.environ.get("HEAD_SHA", "HEAD")
    if not base:
        print("BASE_SHA is required for the tooling scope check", file=sys.stderr)
        return 2
    error = scope_error(changed_paths(base, head), os.environ.get("MAGIK2_TOOLING_PR") == "1")
    if error:
        print(error, file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
