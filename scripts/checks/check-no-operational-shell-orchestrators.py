#!/usr/bin/env python3
# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

"""Prevent shell device/build/deploy/profile orchestration from returning."""

from __future__ import annotations

import re
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]
FORBIDDEN_ENTRYPOINTS = {
    "scripts/mister",
    "scripts/magik-mode.sh",
    "scripts/run-rust.sh",
    "apps/mister/build-arm.sh",
    "apps/mister/build-arm64-apple-container.sh",
}
FORBIDDEN_NAMES = {
    "device-release-acceptance.sh",
    "device-startup-reveal-acceptance.sh",
    "device-launch-return-smoke.sh",
    "mister-video-mode-test.sh",
    "profile-first-scan.sh",
    "profile-first-preview.sh",
    "profile-preview-scroll.sh",
    "profile-arcade-scroll.sh",
    "bench-toolchain.sh",
    "deploy-rust.sh",
    "deploy-platform.sh",
}
ORCHESTRATOR_NAME = re.compile(
    r"^(?:deploy|profile|device|audit|diagnostic|acceptance|bench)-.*\.sh$"
)


def authoritative_files() -> list[Path]:
    roots = [
        ROOT / "AGENTS.md",
        ROOT / "agent-cli",
        ROOT / "apps",
        ROOT / "docs",
        ROOT / "documentation",
        ROOT / "mister",
        ROOT / "scripts",
        ROOT / ".github",
    ]
    files: list[Path] = []
    for root in roots:
        candidates = [root] if root.is_file() else root.rglob("*")
        for path in candidates:
            if not path.is_file() or any(
                part in {"target", "node_modules", "build", "dist"} for part in path.parts
            ):
                continue
            relative = path.relative_to(ROOT).as_posix()
            if relative in {
                "docs/agents/script-deletion-ledger.md",
                "scripts/checks/check-agent-guidance.py",
                "scripts/checks/check-no-operational-shell-orchestrators.py",
            }:
                continue
            if relative.startswith("docs/performance-review-") or relative.startswith(
                "docs/2026-"
            ):
                continue
            files.append(path)
    return files


def main() -> int:
    failures: list[str] = []
    for relative in FORBIDDEN_ENTRYPOINTS:
        if (ROOT / relative).exists():
            failures.append(f"retired entrypoint exists: {relative}")
    for script in (ROOT / "scripts").glob("*.sh"):
        if ORCHESTRATOR_NAME.match(script.name):
            failures.append(f"shell orchestrator is forbidden: {script.relative_to(ROOT)}")
    for path in authoritative_files():
        try:
            text = path.read_text(encoding="utf-8")
        except UnicodeDecodeError:
            continue
        relative = path.relative_to(ROOT)
        for forbidden in (*FORBIDDEN_ENTRYPOINTS, *FORBIDDEN_NAMES):
            if forbidden in text:
                failures.append(f"{relative} references retired interface {forbidden}")
    if failures:
        raise SystemExit("\n".join(sorted(set(failures))))
    print("operational shell ownership check ok")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
