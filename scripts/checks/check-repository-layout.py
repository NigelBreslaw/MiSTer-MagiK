#!/usr/bin/env python3
# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]

obsolete = (
    "magik-gui",
    "desktop",
    "kernel",
    "fpga",
    "latch-contract",
    "scanout-contract",
    "framebuffer-stream",
    "agent-protocol",
    "media-contract",
    "tools/mister",
    "tools/magik-agent",
)
required = (
    "apps/mister",
    "apps/desktop",
    "crates/magik-core",
    "crates/catalog",
    "crates/framebuffer-stream",
    "crates/agent-protocol",
    "crates/media-contract",
    "mister/platform/kernel/scanout-slots",
    "mister/platform/fpga/menu-vblank-latch",
    "mister/platform/contracts/latch",
    "mister/platform/contracts/scanout",
    "mister/platform/runtime",
    "mister/tools/agent",
)

errors = [
    f"obsolete source path still exists: {path}"
    for path in obsolete
    if (ROOT / path).exists()
]
errors.extend(
    f"required source path is missing: {path}"
    for path in required
    if not (ROOT / path).exists()
)
if errors:
    raise SystemExit("\n".join(errors))

print("repository layout contract ok")
