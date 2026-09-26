"""Platform preparation and offline FPGA evidence, independent of the device client."""

from __future__ import annotations

import argparse
import os
import subprocess
import sys
from pathlib import Path
from . import fpga_local


def main(root: Path, argv: list[str]) -> int:
    parser = argparse.ArgumentParser(prog="scripts/magik-platform fpga")
    commands = parser.add_subparsers(dest="action", required=True)
    setup = commands.add_parser("setup")
    setup.add_argument("--local-root", type=Path)
    signoff = commands.add_parser("signoff")
    signoff.add_argument("--local-root", type=Path)
    signoff.add_argument("--menu-source", type=Path)
    for variant in ("stock", "baseline", "patched"):
        signoff.add_argument("--" + variant, type=Path, action="append")
    args = parser.parse_args(argv)
    if args.action == "setup":
        environment = dict(
            os.environ,
            MISTER_FPGA_LOCAL_ROOT=str(fpga_local.local_root(root, args.local_root)),
        )
        subprocess.run(
            [str(root / "scripts/install-quartus-lite-apple-container.sh")],
            cwd=root,
            env=environment,
            check=True,
            timeout=7200,
        )
    else:
        if not any(
            getattr(args, variant) for variant in ("stock", "baseline", "patched")
        ):
            try:
                return fpga_local.signoff(
                    root, fpga_local.local_root(root, args.local_root), args.menu_source
                )
            except ValueError as error:
                parser.error(str(error))
        if not all(
            getattr(args, variant) for variant in ("stock", "baseline", "patched")
        ):
            parser.error("report auditing requires all three variants")
        # Explicit report arguments retain the offline evidence reader.
        command = [
            sys.executable,
            str(root / "scripts/checks/check-fpga-quartus-delta.py"),
        ]
        profile = (
            (root / "mister/platform/fpga/menu-vblank-latch/local-signoff-profile.txt")
            .read_text()
            .strip()
        )
        command.extend(fpga_local.PROFILES[profile])
        for variant in ("stock", "baseline", "patched"):
            for path in getattr(args, variant):
                command.extend(["--" + variant, str(path)])
        subprocess.run(command, cwd=root, check=True, timeout=60)
    return 0
