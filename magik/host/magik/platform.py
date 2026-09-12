"""Thin attended platform entrypoint sharing native discovery and publication."""

from __future__ import annotations
import argparse
import time
from pathlib import Path
from .results import create_run, finalize, source_context
from .token_store import state_root
from .publication import platform, publish


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
    development = commands.add_parser("development-618")
    development.add_argument("--tag", required=True)
    development.add_argument("--attended", action="store_true", required=True)
    development.add_argument("--activate-fpga", action="store_true", required=True)
    recover = commands.add_parser("restore")
    recover.add_argument("--stage", required=True)
    recover.add_argument("--attended", action="store_true", required=True)
    args = parser.parse_args(argv)
    if args.kind == "fpga-install":
        args.kind = "fpga"
    started = time.monotonic()
    run_kind = "platform" if args.kind == "development-618" else args.kind
    run = create_run(state_root() / "results", run_kind, source_context("automatic"))
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
        elif args.kind == "development-618":
            result = install_development_618(args, run)
        else:
            result = platform(args, run)
        finalize(run, 0, int((time.monotonic() - started) * 1000))
        return result
    except Exception as error:
        finalize(run, 2, int((time.monotonic() - started) * 1000))
        print(f"{error}; evidence: {run}")
        return 2


def install_development_618(arguments, run):
    """Install one verified prerelease without changing the normal update queue."""
    from .cli import connect_agent
    from .client import AgentError
    from .update_deploy import ensure_service_boot, prepare, state
    from .updates import development_platform_618
    import json
    import tempfile

    release = development_platform_618(arguments.tag)
    with tempfile.TemporaryDirectory(prefix="magik-development-618-") as temporary:
        files = prepare({"platform": release}, Path(temporary))
        agent, _ = connect_agent(
            run, {"publication-v1", "platform-publication-v1", "service-boot-v1"}
        )
        current = state(agent)
        if (
            current["configured_main"] != "MiSTer_MagiKDev"
            or current["running"].get("executable_path")
            != "/media/fat/MiSTer_MagiKDev"
        ):
            raise AgentError(
                "Linux 6.18 development installation requires running and selected Dev mode"
            )
        ensure_service_boot(agent)
        print(
            json.dumps(
                publish(
                    agent,
                    run,
                    files,
                    kind="platform",
                    layout="dev",
                    attended=True,
                    activate_fpga=True,
                ),
                indent=2,
            )
        )
    return 0
