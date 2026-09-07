"""Thin attended platform entrypoint sharing native discovery and publication."""

from __future__ import annotations
import argparse
import time
from pathlib import Path
from .results import create_run, finalize, source_context
from .token_store import state_root
from .publication import platform


def main(argv=None):
    parser = argparse.ArgumentParser(prog="scripts/magik-platform")
    commands = parser.add_subparsers(dest="kind", required=True)
    for name in ("platform", "local-main", "fpga-install"):
        command = commands.add_parser(name)
        command.add_argument(
            "--root",
            type=Path,
            required=True,
            help="prepared artifact tree relative to /media/fat",
        )
        command.add_argument(
            "--layout",
            choices=("dev",) if name != "platform" else ("dev", "public"),
            default="dev",
        )
        command.add_argument("--attended", action="store_true", required=True)
        if name in {"platform", "fpga-install"}:
            command.add_argument("--activate-fpga", action="store_true", required=True)
        if name == "fpga-install":
            command.add_argument("--signoff-report", type=Path, required=True)
        if name == "local-main":
            command.add_argument("--source-checkout", type=Path, required=True)
    recover = commands.add_parser("restore")
    recover.add_argument("--stage", required=True)
    recover.add_argument("--attended", action="store_true", required=True)
    args = parser.parse_args(argv)
    if args.kind == "fpga-install":
        args.kind = "fpga"
    started = time.monotonic()
    run = create_run(state_root() / "results", args.kind, source_context("automatic"))
    try:
        if args.kind == "restore":
            from .cli import connect_agent
            from .client import AgentError
            import json

            agent, _ = connect_agent(run, {"publication-v1", "platform-publication-v1"})
            reply, _ = agent._request(
                "publication-control",
                {"stage": args.stage, "action": "restore", "attended": True},
                attempts=1,
                timeout=60,
            )
            (run / "restoration.json").write_text(
                json.dumps(dict(reply.fields), indent=2) + "\n"
            )
            if reply.operation != "publication-complete":
                raise AgentError.from_fields(reply.fields)
            print(
                "Previous artifacts restored. An explicit attended reboot is required to activate them."
            )
            result = 0
        else:
            result = platform(args, run)
        finalize(run, 0, int((time.monotonic() - started) * 1000))
        return result
    except Exception as error:
        finalize(run, 2, int((time.monotonic() - started) * 1000))
        print(f"{error}; evidence: {run}")
        return 2
