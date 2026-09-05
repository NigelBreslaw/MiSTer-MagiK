#!/usr/bin/env python3
"""Require an explicit tooling PR marker when changing the 2.0 core."""

from __future__ import annotations

import os
import subprocess
import sys

CORE_PREFIXES = ("magik2/agent/", "magik2/host/magik2/", "magik2/docs/")


def changed_paths(base: str, head: str) -> list[str]:
    output = subprocess.check_output(["git", "diff", "--name-only", f"{base}...{head}"], text=True)
    return output.splitlines()


def main() -> int:
    base = os.environ.get("BASE_SHA")
    head = os.environ.get("HEAD_SHA", "HEAD")
    if not base:
        print("BASE_SHA is required for the tooling scope check", file=sys.stderr)
        return 2
    core = [path for path in changed_paths(base, head) if path.startswith(CORE_PREFIXES)]
    if core and os.environ.get("MAGIK2_TOOLING_PR") != "1":
        print("MagiK 2.0 core changes require a dedicated tooling PR (set MAGIK2_TOOLING_PR=1).", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
