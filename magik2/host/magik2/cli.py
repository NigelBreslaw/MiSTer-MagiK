"""Thin public command interface. Device operations are native-only."""

from __future__ import annotations

import argparse
import hashlib
import os
import time
import webbrowser
from pathlib import Path

from .bootstrap import BootstrapError, SshBootstrap
from .build import ensure_arm_probe, ensure_arm_agent, ensure_arm_package
from .client import AgentError, NativeAgent
from .compatibility import AgentStatus
from .results import append_event, create_run, source_context
from .scenarios import motion, smoke
from .testing import fresh_session
from .token_store import TokenStore, state_root
from .viewer import serve


STATUS_CAPABILITIES = {"status"}
STOP_CAPABILITIES = {"status", "lifecycle-v1"}
REQUIRED_AGENT_CAPABILITIES = {"status", "upload-v1", "start-artifact", "request-replay-v1"}
CHECK_AGENT_CAPABILITIES = REQUIRED_AGENT_CAPABILITIES | {"metrics-v1", "test-bridge-v1", "test-deadline-v2"}
WATCH_AGENT_CAPABILITIES = {"status", "metrics-v1", "watch-v1"}
PROFILE_AGENT_CAPABILITIES = CHECK_AGENT_CAPABILITIES | {"artifacts-v1"}


def agent_binary_path() -> Path:
    return ensure_arm_agent()


def main() -> int:
    parser = argparse.ArgumentParser(prog="scripts/magik2")
    subcommands = parser.add_subparsers(dest="command", required=True)
    subcommands.add_parser("deploy")
    build_command = subcommands.add_parser("build")
    build_command.add_argument("target", choices=("agent", "probe"))
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
    if arguments.command == "build":
        package = Path(__file__).resolve().parents[2] / arguments.target
        built = ensure_arm_package(package, package / "target/magik2-build.json")
        print(built.artifact)
        return 0
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
        agent, status = connect_agent(run, STATUS_CAPABILITIES)
    except (BootstrapError, AgentError, OSError, RuntimeError) as error:
        append_event(run, {"phase": "status", "outcome": "failed", "error": type(error).__name__})
        print(f"magik2 status: native agent unavailable ({type(error).__name__}) (result: {run})", file=os.sys.stderr)
        return 2
    append_event(run, {"phase": "status", "outcome": "passed", "identity": status.identity})
    print(
        f"identity={status.identity} running={status.fields.get('running', False)} "
        f"legacy_agent_running={status.fields.get('legacy_agent_running', 'unknown')} "
        f"capabilities={','.join(sorted(status.capabilities))}"
    )
    return 0


def deploy(_arguments: argparse.Namespace, run: Path) -> int:
    started = time.monotonic()
    try:
        agent, status = connect_agent(run)
        append_event(run, {"phase": "connect", "elapsed_ms": int((time.monotonic() - started) * 1_000)})
        if ensure_probe(agent, status, run):
            append_event(run, {"phase": "complete", "outcome": "no-op", "elapsed_ms": int((time.monotonic() - started) * 1_000)})
            print(f"magik2 deploy: probe already ready (result: {run})")
            return 0
        append_event(run, {"phase": "complete", "outcome": "started", "elapsed_ms": int((time.monotonic() - started) * 1_000)})
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
    primary_error: Exception | None = None
    cleanup_errors: list[str] = []
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
    except Exception as error:
        primary_error = error
        append_event(run, {"phase": "check", "outcome": "failed", "error": str(error)})
    finally:
        if profile_id is not None and "agent" in locals():
            for name in ("profile.folded", "flamegraph.svg"):
                try:
                    (run / name).write_bytes(agent.read_profile_artifact(profile_id, name))
                    append_event(run, {"phase": "profile-artifact", "outcome": "retained", "name": name, "instrumented": True})
                except (AgentError, OSError) as error:
                    append_event(run, {"phase": "profile-artifact", "outcome": "unavailable", "name": name, "error": str(error)})
                    cleanup_errors.append(f"profile artifact {name}: {error}")
        if session_started:
            try:
                agent.start(restart=True)
                append_event(run, {"phase": "persistent-restart", "outcome": "passed"})
            except (BootstrapError, AgentError, OSError, UnboundLocalError) as error:
                append_event(run, {"phase": "persistent-restart", "outcome": "failed", "error": type(error).__name__})
                cleanup_errors.append(f"persistent restart: {type(error).__name__}")
    if cleanup_errors:
        append_event(run, {"phase": "cleanup", "outcome": "failed", "errors": cleanup_errors})
    else:
        append_event(run, {"phase": "cleanup", "outcome": "passed"})
    if primary_error is not None or cleanup_errors:
        message = str(primary_error) if primary_error is not None else "; ".join(cleanup_errors)
        print(f"magik2 check: {message} (result: {run})", file=os.sys.stderr)
        return 2
    append_event(run, {"phase": "check", "outcome": "passed"})
    print(f"magik2 check: passed ({','.join(scenarios)}) (result: {run})")
    return 0


def stop(run: Path) -> int:
    try:
        agent, _ = connect_agent(run, STOP_CAPABILITIES)
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
    append_event(
        run,
        {
            "phase": "build",
            "outcome": "prebuilt" if built.prebuilt else "rebuilt" if built.rebuilt else "reused",
            "elapsed_ms": built.elapsed_ms,
        },
    )
    artifact = built.artifact
    payload = artifact.read_bytes()
    artifact_hash = hashlib.sha256(payload).hexdigest()
    healthy = status.fields.get("running") is True and status.fields.get("running_sha256") == artifact_hash and status.fields.get("ready") is True
    if healthy:
        append_event(run, {"phase": "deploy", "outcome": "no-op", "bytes": 0})
        return True
    changed = status.fields.get("artifact_sha256") != artifact_hash
    if changed:
        upload_started = time.monotonic()
        append_event(run, {"phase": "upload", "bytes": len(payload)})
        agent.upload("probe", payload)
        upload_elapsed_ms = max(1, int((time.monotonic() - upload_started) * 1_000))
        append_event(
            run,
            {
                "phase": "upload-complete",
                "elapsed_ms": upload_elapsed_ms,
                "bytes_per_second": len(payload) * 1_000 // upload_elapsed_ms,
            },
        )
    start_started = time.monotonic()
    append_event(run, {"phase": "start", "restart": status.fields.get("running") is True})
    agent.start(expected_sha256=artifact_hash, restart=status.fields.get("running") is True)
    append_event(run, {"phase": "start-complete", "elapsed_ms": int((time.monotonic() - start_started) * 1_000)})
    return False


def wait_for_agent(agent: NativeAgent, required: set[str], timeout: float = 15) -> AgentStatus:
    deadline = time.monotonic() + timeout
    last: object = "not ready"
    while time.monotonic() < deadline:
        try:
            status = agent.status()
            if status.supports(required):
                return status
            last = "missing capabilities: " + ", ".join(sorted(required - status.capabilities))
        except AgentError:
            raise  # Authentication never authorizes replacement of credentials.
        except (OSError, RuntimeError) as error:
            last = error
        time.sleep(0.1)
    raise AgentError(f"native agent did not become ready: {last}")


def connect_agent(run: Path, required: set[str] = REQUIRED_AGENT_CAPABILITIES) -> tuple[NativeAgent, AgentStatus]:
    device = os.environ["MISTER_IP"]
    store = TokenStore(state_root(), device)
    token = store.load()
    if not token:
        token = SshBootstrap.from_environment().native_token()
        if token:
            store.save(token)
    agent = NativeAgent(device, token) if token else None
    status = None
    if agent is not None:
        try:
            status = agent.status()
        except AgentError:
            raise
        except (OSError, RuntimeError):
            pass
    repair = os.environ.get("MISTER_MAGIK2_REPAIR") == "1"
    if status is not None and status.supports(required) and not repair:
        append_event(run, {"phase": "agent", "identity": status.identity, "capabilities": sorted(status.capabilities)})
        return agent, status
    binary = agent_binary_path()  # Build only after proving installed support is insufficient.
    if status is not None and status.supports({"agent-update-v1"}) and not repair:
        append_event(run, {"phase": "native-agent-update", "outcome": "requested"})
        payload = binary.read_bytes()
        try:
            agent.upgrade_agent(payload)
        except (OSError, RuntimeError) as error:
            if isinstance(error, AgentError):
                raise
            append_event(run, {"phase": "native-agent-update", "outcome": "reply-lost", "error": str(error)})
        # Never blindly repeat an update after losing its acknowledgement.
        status = wait_for_agent(agent, required)
    else:
        token = SshBootstrap.from_environment().install_and_start(binary)
        store.save(token)
        agent = NativeAgent(device, token)
        status = wait_for_agent(agent, required)
        append_event(run, {"phase": "bootstrap", "outcome": "passed"})
    return agent, status


if __name__ == "__main__":
    raise SystemExit(main())
