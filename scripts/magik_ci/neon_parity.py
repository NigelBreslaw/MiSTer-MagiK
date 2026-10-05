# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

"""Native NEON correctness on ARMv7 or AArch64; never a timing benchmark."""

from __future__ import annotations

import platform
import subprocess
import tempfile
from pathlib import Path


def run(repository: Path, compiler: str) -> None:
    machine = platform.machine().lower()
    if machine not in {"arm64", "aarch64", "armv7l", "armv8l"}:
        raise RuntimeError("NEON parity requires a native ARMv7 or AArch64 host")
    source = repository / "crates/framebuffer-scenes/tests/launcher_neon_parity.c"
    with tempfile.TemporaryDirectory(prefix="magik-neon-parity-") as temporary:
        for fast in (False, True):
            executable = Path(temporary) / ("fast" if fast else "current")
            command = [compiler, "-O3", "-std=c11"]
            if machine in {"armv7l", "armv8l"}:
                command += ["-mfpu=neon-vfpv3", "-mfloat-abi=hard"]
            if fast:
                command += ["-DMAGIK_FAST_QUANTISATION"]
            subprocess.run([*command, str(source), "-o", str(executable)], check=True)
            print(f"NEON parity: {'fast' if fast else 'current'}", flush=True)
            subprocess.run([str(executable)], check=True)
