"""Thin public command interface. Device operations are native-only."""

from __future__ import annotations

import argparse
import os
import time
from pathlib import Path

from .bootstrap import BootstrapError, SshBootstrap
from .client import AgentError, NativeAgent
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
    if arguments.command != "status":
        print(f"magik2 {arguments.command}: lifecycle/probe adapter is not installed yet (result: {run})", file=os.sys.stderr)
        return 2
    try:
        bootstrap = SshBootstrap.from_environment()
        token = bootstrap.native_token()
        status = NativeAgent(os.environ["MISTER_IP"], token).status()
        if not status.supports({"status", "upload-v1", "lifecycle-v1"}):
            raise AgentError("missing-required-capability")
    except (AgentError, OSError):
        try:
            agent_binary = Path(__file__).resolve().parents[2] / "agent" / "target" / "armv7-unknown-linux-gnueabihf" / "release" / "mister-magik2-agent"
            token = bootstrap.install_and_start(agent_binary)
            time.sleep(1)
            status = NativeAgent(os.environ["MISTER_IP"], token).status()
            append_event(run, {"phase": "bootstrap", "outcome": "passed"})
        except (BootstrapError, AgentError, OSError) as error:
            append_event(run, {"phase": "status", "outcome": "failed", "error": type(error).__name__})
            print(f"magik2 status: native agent unavailable ({type(error).__name__}) (result: {run})", file=os.sys.stderr)
            return 2
    except BootstrapError as error:
        append_event(run, {"phase": "status", "outcome": "failed", "error": type(error).__name__})
        print(f"magik2 status: native agent token unavailable ({type(error).__name__}) (result: {run})", file=os.sys.stderr)
        return 2
    append_event(run, {"phase": "status", "outcome": "passed", "identity": status.identity})
    print(f"identity={status.identity} capabilities={','.join(sorted(status.capabilities))}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
