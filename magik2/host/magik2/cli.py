"""Thin public command interface. Device operations are native-only."""

from __future__ import annotations

import argparse
import os
from pathlib import Path

from .results import append_event, create_run


def main() -> int:
    parser = argparse.ArgumentParser(prog="scripts/magik2")
    subcommands = parser.add_subparsers(dest="command", required=True)
    subcommands.add_parser("deploy")
    check = subcommands.add_parser("check")
    check.add_argument("scenario", choices=("smoke", "motion"), nargs="?")
    check.add_argument("--profile", action="store_true")
    subcommands.add_parser("watch")
    subcommands.add_parser("status")
    subcommands.add_parser("stop")
    arguments = parser.parse_args()

    output_root = Path(os.environ.get("MISTER_MAGIK2_RESULTS", "build/magik2-results"))
    run = create_run(output_root, arguments.command, {"mister_ip": os.environ.get("MISTER_IP", "")})
    append_event(run, {"phase": "requested", "command": arguments.command})
    if not os.environ.get("MISTER_IP"):
        print("MISTER_IP is required; no legacy transport was attempted.", file=os.sys.stderr)
        return 2
    print(f"magik2 {arguments.command}: device bootstrap/probe adapter is not installed yet (result: {run})", file=os.sys.stderr)
    return 2


if __name__ == "__main__":
    raise SystemExit(main())
