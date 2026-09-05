"""Thin public command interface. Device operations are native-only."""

from __future__ import annotations

import argparse
import hashlib
import os
import time
import webbrowser
from pathlib import Path

from .bootstrap import BootstrapError, SshBootstrap
from .build import ensure_arm_probe
from .client import AgentError, NativeAgent
from .compatibility import AgentStatus
from .results import append_event, create_run, source_context
from .scenarios import motion, smoke
from .testing import fresh_session
from .token_store import TokenStore
from .viewer import serve


REQUIRED_AGENT_CAPABILITIES = {"status", "upload-v1", "lifecycle-v1"}
CHECK_AGENT_CAPABILITIES = REQUIRED_AGENT_CAPABILITIES | {"metrics-v1", "test-bridge-v1"}
WATCH_AGENT_CAPABILITIES = REQUIRED_AGENT_CAPABILITIES | {"metrics-v1", "watch-v1"}
PROFILE_AGENT_CAPABILITIES = CHECK_AGENT_CAPABILITIES | {"artifacts-v1"}


def agent_binary_path() -> Path:
    return Path(__file__).resolve().parents[2] / "agent" / "target" / "armv7-unknown-linux-gnueabihf" / "release" / "mister-magik2-agent"


def main() -> int:
    parser = argparse.ArgumentParser(prog="scripts/magik2")
    subcommands = parser.add_subparsers(dest="command", required=True)
    subcommands.add_parser("deploy")
    check_command = subcommands.add_parser("check")
    check_command.add_argument("scenario", choices=("smoke", "motion"), nargs="?")
    check_command.add_argument("--profile", action="store_true")
    subcommands.add_parser("watch")
    subcommands.add_parser("status")
    subcommands.add_parser("stop")
    arguments = parser.parse_args()

    output_root = Path(os.environ.get("MISTER_MAGIK2_RESULTS", "build/magik2-results"))
    run = create_run(output_root, arguments.command, source_context(os.environ.get("MISTER_IP", "")))
    append_event(run, {"phase": "requested", "command": arguments.command})
    if not os.environ.get("MISTER_IP"):
        print("MISTER_IP is required; no legacy transport was attempted.", file=os.sys.stderr)
        return 2
    if arguments.command == "deploy":
        return deploy(arguments, run)
    if arguments.command == "stop":
        return stop(run)
    if arguments.command == "check":
        return check(arguments, run)
    if arguments.command == "watch":
        return watch(run)
    if arguments.command != "status":
        print(f"magik2 {arguments.command}: not implemented yet (result: {run})", file=os.sys.stderr)
        return 2
    try:
        agent, status = connect_agent(run)
    except (BootstrapError, AgentError, OSError, RuntimeError) as error:
        append_event(run, {"phase": "status", "outcome": "failed", "error": type(error).__name__})
        print(f"magik2 status: native agent unavailable ({type(error).__name__}) (result: {run})", file=os.sys.stderr)
        return 2
    append_event(run, {"phase": "status", "outcome": "passed", "identity": status.identity})
    print(f"identity={status.identity} running={status.fields.get('running', False)} capabilities={','.join(sorted(status.capabilities))}")
    return 0


def deploy(_arguments: argparse.Namespace, run: Path) -> int:
    started = time.monotonic()
    try:
        agent, status = connect_agent(run)
        append_event(run, {"phase": "connect", "elapsed_ms": int((time.monotonic() - started) * 1_000)})
        if ensure_probe(agent, status, run):
            print(f"magik2 deploy: probe already ready (result: {run})")
            return 0
        append_event(run, {"phase": "complete", "outcome": "started"})
    except (BootstrapError, AgentError, OSError, RuntimeError) as error:
        append_event(run, {"phase": "failed", "error": type(error).__name__})
        print(f"magik2 deploy: {error} (result: {run})", file=os.sys.stderr)
        return 2
    print(f"magik2 deploy: probe started (result: {run})")
    return 0


def check(arguments: argparse.Namespace, run: Path) -> int:
    scenarios = (arguments.scenario,) if arguments.scenario else ("smoke", "motion")
    if arguments.profile and scenarios != ("motion",):
        append_event(run, {"phase": "profile", "outcome": "invalid-selection"})
        print(f"magik2 check: --profile requires the motion scenario (result: {run})", file=os.sys.stderr)
        return 2
    profile_id = "profile" if arguments.profile else None
    session_started = False
    try:
        agent, status = connect_agent(run, PROFILE_AGENT_CAPABILITIES if profile_id is not None else CHECK_AGENT_CAPABILITIES)
        ensure_probe(agent, status, run)
        with fresh_session(agent, profile_id=profile_id) as application:
            session_started = True
            if "smoke" in scenarios:
                append_event(run, {"phase": "smoke", "outcome": "started"})
                append_event(run, {"phase": "smoke", "outcome": "passed", **smoke(application, run / "smoke.png")})
            if "motion" in scenarios:
                append_event(run, {"phase": "motion", "outcome": "started"})
                measurement = motion(application, agent)
                append_event(run, {"phase": "motion", "outcome": "measured", **measurement})
                if not measurement["physical_evidence_valid"]:
                    raise AssertionError("motion has no validated physical-presentation evidence")
    except (AssertionError, BootstrapError, AgentError, OSError, RuntimeError) as error:
        append_event(run, {"phase": "check", "outcome": "failed", "error": str(error)})
        print(f"magik2 check: {error} (result: {run})", file=os.sys.stderr)
        return 2
    finally:
        if profile_id is not None and "agent" in locals():
            for name in ("profile.folded", "flamegraph.svg"):
                try:
                    (run / name).write_bytes(agent.read_profile_artifact(profile_id, name))
                    append_event(run, {"phase": "profile-artifact", "outcome": "retained", "name": name, "instrumented": True})
                except (AgentError, OSError) as error:
                    append_event(run, {"phase": "profile-artifact", "outcome": "unavailable", "name": name, "error": str(error)})
        if session_started:
            try:
                agent.start(restart=True)
                append_event(run, {"phase": "persistent-restart", "outcome": "passed"})
            except (BootstrapError, AgentError, OSError, UnboundLocalError) as error:
                append_event(run, {"phase": "persistent-restart", "outcome": "failed", "error": type(error).__name__})
    append_event(run, {"phase": "check", "outcome": "passed"})
    print(f"magik2 check: passed ({','.join(scenarios)}) (result: {run})")
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


def watch(run: Path) -> int:
    try:
        agent, _ = connect_agent(run, WATCH_AGENT_CAPABILITIES)
        server, url = serve(agent)
        append_event(run, {"phase": "watch", "outcome": "started", "url": url})
        webbrowser.open(url)
        print(f"magik2 watch: {url} (Ctrl-C to stop; result: {run})")
        server.serve_forever()
    except KeyboardInterrupt:
        append_event(run, {"phase": "watch", "outcome": "stopped"})
        return 0
    except (BootstrapError, AgentError, OSError, RuntimeError) as error:
        append_event(run, {"phase": "watch", "outcome": "failed", "error": str(error)})
        print(f"magik2 watch: {error} (result: {run})", file=os.sys.stderr)
        return 2
    finally:
        if "server" in locals():
            server.server_close()


def ensure_probe(agent: NativeAgent, status: AgentStatus, run: Path) -> bool:
    probe_root = Path(__file__).resolve().parents[2] / "probe"
    built = ensure_arm_probe(
        probe_root,
        Path(os.environ.get("MISTER_MAGIK2_STATE", "build/magik2-state")) / "probe-build.json",
    )
    append_event(run, {"phase": "build", "outcome": "rebuilt" if built.rebuilt else "reused", "elapsed_ms": built.elapsed_ms})
    artifact = built.artifact
    payload = artifact.read_bytes()
    artifact_hash = hashlib.sha256(payload).hexdigest()
    healthy = status.fields.get("running") is True and status.fields.get("artifact_sha256") == artifact_hash
    if healthy:
        append_event(run, {"phase": "deploy", "outcome": "no-op", "bytes": 0})
        return True
    changed = status.fields.get("artifact_sha256") != artifact_hash
    if changed:
        upload_started = time.monotonic()
        append_event(run, {"phase": "upload", "bytes": len(payload)})
        agent.upload("probe", payload)
        append_event(run, {"phase": "upload-complete", "elapsed_ms": int((time.monotonic() - upload_started) * 1_000)})
    start_started = time.monotonic()
    append_event(run, {"phase": "start", "restart": status.fields.get("running") is True})
    agent.start(restart=status.fields.get("running") is True)
    append_event(run, {"phase": "start-complete", "elapsed_ms": int((time.monotonic() - start_started) * 1_000)})
    return False


def connect_agent(
    run: Path, required: set[str] = REQUIRED_AGENT_CAPABILITIES
) -> tuple[NativeAgent, AgentStatus]:
    """Use SSH only when native discovery or repair is genuinely unavailable."""
    device = os.environ["MISTER_IP"]
    store = TokenStore(Path(os.environ.get("MISTER_MAGIK2_STATE", "build/magik2-state")), device)
    agent_binary = agent_binary_path()
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
            repair_requested = os.environ.get("MISTER_MAGIK2_REPAIR") == "1"
            if status.supports(required) and not repair_requested:
                return agent, status
            if not repair_requested and status.supports({"agent-update-v1"}):
                if not agent_binary.is_file():
                    raise AgentError("ARM native-agent artifact is unavailable")
                append_event(run, {"phase": "native-agent-update", "outcome": "requested"})
                agent.upgrade_agent(agent_binary.read_bytes())
                time.sleep(1)
                status = NativeAgent(device, token).status()
                if status.supports(required):
                    append_event(run, {"phase": "native-agent-update", "outcome": "passed"})
                    return NativeAgent(device, token), status
    bootstrap = SshBootstrap.from_environment()
    token = bootstrap.install_and_start(agent_binary)
    store.save(token)
    time.sleep(1)
    agent = NativeAgent(device, token)
    status = agent.status()
    if not status.supports(required):
        raise AgentError("missing-required-capability-after-repair")
    append_event(run, {"phase": "bootstrap", "outcome": "passed"})
    return agent, status


if __name__ == "__main__":
    raise SystemExit(main())
