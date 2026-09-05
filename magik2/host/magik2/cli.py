"""Thin public command interface. Device operations are native-only."""

from __future__ import annotations

import argparse
import hashlib
import os
import time
from pathlib import Path

from .bootstrap import BootstrapError, SshBootstrap
from .client import AgentError, NativeAgent
from .compatibility import AgentStatus
from .results import append_event, create_run
from .token_store import TokenStore


REQUIRED_AGENT_CAPABILITIES = {"status", "upload-v1", "lifecycle-v1"}


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
    if arguments.command == "deploy":
        return deploy(arguments, run)
    if arguments.command == "stop":
        return stop(run)
    if arguments.command != "status":
        print(f"magik2 {arguments.command}: not implemented yet (result: {run})", file=os.sys.stderr)
        return 2
    try:
        agent, status = connect_agent(run)
    except (BootstrapError, AgentError, OSError) as error:
        append_event(run, {"phase": "status", "outcome": "failed", "error": type(error).__name__})
        print(f"magik2 status: native agent unavailable ({type(error).__name__}) (result: {run})", file=os.sys.stderr)
        return 2
    append_event(run, {"phase": "status", "outcome": "passed", "identity": status.identity})
    print(f"identity={status.identity} running={status.fields.get('running', False)} capabilities={','.join(sorted(status.capabilities))}")
    return 0


def deploy(_arguments: argparse.Namespace, run: Path) -> int:
    artifact = Path(__file__).resolve().parents[2] / "probe" / "target" / "armv7-unknown-linux-gnueabihf" / "release" / "mister-magik2-probe"
    if not artifact.is_file():
        print(f"magik2 deploy: ARM probe artifact is unavailable (result: {run})", file=os.sys.stderr)
        return 2
    try:
        agent, status = connect_agent(run)
        payload = artifact.read_bytes()
        artifact_hash = hashlib.sha256(payload).hexdigest()
        healthy = status.fields.get("running") is True and status.fields.get("artifact_sha256") == artifact_hash
        if healthy:
            append_event(run, {"phase": "complete", "outcome": "no-op", "bytes": 0})
            print(f"magik2 deploy: probe already ready (result: {run})")
            return 0
        changed = status.fields.get("artifact_sha256") != artifact_hash
        if changed:
            append_event(run, {"phase": "upload", "bytes": len(payload)})
            agent.upload("probe", payload)
        append_event(run, {"phase": "start", "restart": status.fields.get("running") is True})
        agent.start(restart=status.fields.get("running") is True)
        append_event(run, {"phase": "complete", "outcome": "started"})
    except (BootstrapError, AgentError, OSError) as error:
        append_event(run, {"phase": "failed", "error": type(error).__name__})
        print(f"magik2 deploy: {type(error).__name__} (result: {run})", file=os.sys.stderr)
        return 2
    print(f"magik2 deploy: probe started (result: {run})")
    return 0


def stop(run: Path) -> int:
    try:
        agent, _ = connect_agent(run)
        agent.stop()
    except (BootstrapError, AgentError, OSError) as error:
        append_event(run, {"phase": "stop", "outcome": "failed", "error": type(error).__name__})
        print(f"magik2 stop: {type(error).__name__} (result: {run})", file=os.sys.stderr)
        return 2
    append_event(run, {"phase": "stop", "outcome": "passed"})
    print(f"magik2 stop: launcher resume requested (result: {run})")
    return 0


def connect_agent(run: Path) -> tuple[NativeAgent, AgentStatus]:
    """Use SSH only when native discovery or repair is genuinely unavailable."""
    device = os.environ["MISTER_IP"]
    store = TokenStore(Path(os.environ.get("MISTER_MAGIK2_STATE", "build/magik2-state")), device)
    token = store.load()
    if token:
        agent = NativeAgent(device, token)
        try:
            status = agent.status()
        except AgentError:
            raise
        except OSError:
            pass
        else:
            if status.supports(REQUIRED_AGENT_CAPABILITIES) and os.environ.get("MISTER_MAGIK2_REPAIR") != "1":
                return agent, status
    bootstrap = SshBootstrap.from_environment()
    agent_binary = Path(__file__).resolve().parents[2] / "agent" / "target" / "armv7-unknown-linux-gnueabihf" / "release" / "mister-magik2-agent"
    token = bootstrap.install_and_start(agent_binary)
    store.save(token)
    time.sleep(1)
    agent = NativeAgent(device, token)
    status = agent.status()
    if not status.supports(REQUIRED_AGENT_CAPABILITIES):
        raise AgentError("missing-required-capability-after-repair")
    append_event(run, {"phase": "bootstrap", "outcome": "passed"})
    return agent, status


if __name__ == "__main__":
    raise SystemExit(main())
