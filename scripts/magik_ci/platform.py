"""Platform preparation and offline FPGA evidence, independent of the device client."""

from __future__ import annotations

import argparse
import os
import subprocess
import sys
from pathlib import Path


def main(root: Path, argv: list[str]) -> int:
    parser = argparse.ArgumentParser(prog="scripts/magik-platform fpga")
    commands = parser.add_subparsers(dest="action", required=True)
    setup = commands.add_parser("setup")
    setup.add_argument("--local-root", type=Path, required=True)
    signoff = commands.add_parser("signoff")
    for variant in ("stock", "baseline", "patched"):
        signoff.add_argument("--" + variant, type=Path, action="append", required=True)
    args = parser.parse_args(argv)
    if args.action == "setup":
        environment = dict(
            os.environ, MISTER_FPGA_LOCAL_ROOT=str(args.local_root.resolve())
        )
        subprocess.run(
            [str(root / "scripts/install-quartus-lite-apple-container.sh")],
            cwd=root,
            env=environment,
            check=True,
            timeout=7200,
        )
    else:
        # Evidence reader only: no synthesis, device matrix or aggregate certificate.
        command = [
            sys.executable,
            str(root / "scripts/checks/check-fpga-quartus-delta.py"),
        ]
        for variant in ("stock", "baseline", "patched"):
            for path in getattr(args, variant):
                command.extend(["--" + variant, str(path)])
        subprocess.run(command, cwd=root, check=True, timeout=60)
    return 0
