from __future__ import annotations

import argparse
import json
import sys

import pytest

from magik2 import cli
from magik2.client import AgentError
from magik2.compatibility import AgentStatus
from magik2.results import create_run
from magik2.token_store import TokenStore


def status(identity: str, capabilities: set[str]) -> AgentStatus:
    return AgentStatus.from_response({"identity": identity, "capabilities": sorted(capabilities)})


def configure_native(monkeypatch: pytest.MonkeyPatch, tmp_path, responses: list[AgentStatus | Exception]) -> list[str]:
    monkeypatch.setenv("MISTER_IP", "mister.test")
    monkeypatch.setenv("MISTER_MAGIK2_STATE", str(tmp_path / "state"))
    TokenStore(tmp_path / "state", "mister.test").save("cached-token")
    binary = tmp_path / "agent"
    binary.write_bytes(b"agent")
    monkeypatch.setattr(cli, "agent_binary_path", lambda: binary)
    tokens: list[str] = []

    class FakeAgent:
        def __init__(self, _host: str, token: str) -> None:
            tokens.append(token)

        def status(self) -> AgentStatus:
            reply = responses.pop(0)
            if isinstance(reply, Exception):
                raise reply
            return reply

    monkeypatch.setattr(cli, "NativeAgent", FakeAgent)
    return tokens


def test_branch_clients_keep_a_suitable_agent_despite_identity_changes(monkeypatch: pytest.MonkeyPatch, tmp_path) -> None:
    required = cli.REQUIRED_AGENT_CAPABILITIES
    tokens = configure_native(
        monkeypatch,
        tmp_path,
        [status("branch-a", required), status("branch-b", required | {"future-v1"}), status("branch-a", required)],
    )

    class Bootstrap:
        def native_token(self):
            return None

        def install_and_start(self, _binary) -> str:  # pragma: no cover - must not run
            raise AssertionError("compatible agent must be retained")

    monkeypatch.setattr(cli, "agent_binary_path", lambda: tmp_path / "agent")
    monkeypatch.setattr(cli.SshBootstrap, "from_environment", lambda: Bootstrap())
    run = create_run(tmp_path, "status", {})
    for _ in range(3):
        _agent, discovered = cli.connect_agent(run)
        assert discovered.supports(required)
    assert tokens == ["cached-token", "cached-token", "cached-token"]


def test_missing_capability_bootstraps_once_and_continues(monkeypatch: pytest.MonkeyPatch, tmp_path) -> None:
    required = cli.REQUIRED_AGENT_CAPABILITIES
    tokens = configure_native(monkeypatch, tmp_path, [status("old", {"status"}), status("new", required)])
    installs: list[str] = []

    class Bootstrap:
        def native_token(self):
            return None

        def install_and_start(self, _binary) -> str:
            installs.append("install")
            return "replacement-token"

    monkeypatch.setattr(cli, "agent_binary_path", lambda: tmp_path / "agent")
    monkeypatch.setattr(cli.SshBootstrap, "from_environment", lambda: Bootstrap())
    monkeypatch.setattr(cli.time, "sleep", lambda _seconds: None)
    run = create_run(tmp_path, "status", {})
    _agent, discovered = cli.connect_agent(run)

    assert installs == ["install"]
    assert discovered.supports(required)
    assert tokens == ["cached-token", "replacement-token"]


def test_missing_capability_prefers_a_native_agent_update(monkeypatch: pytest.MonkeyPatch, tmp_path) -> None:
    required = cli.REQUIRED_AGENT_CAPABILITIES
    monkeypatch.setenv("MISTER_IP", "mister.test")
    monkeypatch.setenv("MISTER_MAGIK2_STATE", str(tmp_path / "state"))
    TokenStore(tmp_path / "state", "mister.test").save("cached-token")
    binary = tmp_path / "mister-magik2-agent"
    binary.write_bytes(b"replacement-agent")
    monkeypatch.setattr(cli, "agent_binary_path", lambda: binary)
    statuses = [status("old", {"status", "agent-update-v1"}), status("new", required)]
    uploaded: list[bytes] = []

    class FakeAgent:
        def __init__(self, _host: str, _token: str) -> None:
            pass

        def status(self) -> AgentStatus:
            return statuses.pop(0)

        def upgrade_agent(self, payload: bytes) -> None:
            uploaded.append(payload)

    class Bootstrap:
        def native_token(self):
            return None

        def install_and_start(self, _binary) -> str:  # pragma: no cover - must not run
            raise AssertionError("native update should precede SSH bootstrap")

    monkeypatch.setattr(cli, "NativeAgent", FakeAgent)
    monkeypatch.setattr(cli, "agent_binary_path", lambda: binary)
    monkeypatch.setattr(cli.SshBootstrap, "from_environment", lambda: Bootstrap())
    monkeypatch.setattr(cli.time, "sleep", lambda _seconds: None)
    run = create_run(tmp_path, "status", {})

    assert cli.connect_agent(run)[1].supports(required)
    assert uploaded == [b"replacement-agent"]


def test_absent_agent_bootstraps_and_authentication_failure_does_not(monkeypatch: pytest.MonkeyPatch, tmp_path) -> None:
    required = cli.REQUIRED_AGENT_CAPABILITIES
    configure_native(monkeypatch, tmp_path, [OSError("absent"), status("new", required)])
    installs: list[str] = []

    class Bootstrap:
        def native_token(self):
            return None

        def install_and_start(self, _binary) -> str:
            installs.append("install")
            return "replacement-token"

    monkeypatch.setattr(cli, "agent_binary_path", lambda: tmp_path / "agent")
    monkeypatch.setattr(cli.SshBootstrap, "from_environment", lambda: Bootstrap())
    monkeypatch.setattr(cli.time, "sleep", lambda _seconds: None)
    run = create_run(tmp_path, "status", {})
    assert cli.connect_agent(run)[1].supports(required)
    assert installs == ["install"]

    configure_native(monkeypatch, tmp_path / "auth", [AgentError("authentication-failed")])
    with pytest.raises(AgentError, match="authentication-failed"):
        cli.connect_agent(create_run(tmp_path / "auth", "status", {}))
    assert installs == ["install"]


def test_check_command_dispatches_without_shadowing_the_handler(monkeypatch: pytest.MonkeyPatch, tmp_path) -> None:
    monkeypatch.setenv("MISTER_IP", "mister.test")
    monkeypatch.setenv("MISTER_MAGIK2_RESULTS", str(tmp_path / "results"))
    monkeypatch.setattr(sys, "argv", ["scripts/magik2", "check", "motion", "--profile"])
    received: dict[str, object] = {}

    def handler(arguments, run) -> int:
        received["scenario"] = arguments.scenario
        received["profile"] = arguments.profile
        received["run"] = run
        return 17

    monkeypatch.setattr(cli, "check", handler)
    assert cli.main() == 17
    assert received["scenario"] == "motion"
    assert received["profile"] is True


def test_fresh_worktrees_retrieve_token_without_replacing_compatible_agent(monkeypatch, tmp_path):
    monkeypatch.setenv("MISTER_IP", "mister.test")
    monkeypatch.setenv("XDG_STATE_HOME", str(tmp_path / "shared"))
    monkeypatch.delenv("MISTER_MAGIK2_STATE", raising=False)
    seen = []
    class Bootstrap:
        def native_token(self):
            seen.append("retrieve")
            return "existing-token"
        def install_and_start(self, _binary):
            raise AssertionError("must retain compatible agent")
    class Agent:
        def __init__(self, _host, token):
            assert token == "existing-token"
        def status(self):
            return status("other-branch", cli.REQUIRED_AGENT_CAPABILITIES | {"extra"})
    monkeypatch.setattr(cli.SshBootstrap, "from_environment", lambda: Bootstrap())
    monkeypatch.setattr(cli, "NativeAgent", Agent)
    monkeypatch.setattr(cli, "agent_binary_path", lambda: pytest.fail("compatible agent needs no local build"))
    for name in ("A", "B", "A"):
        checkout = tmp_path / name
        checkout.mkdir(exist_ok=True)
        monkeypatch.chdir(checkout)
        assert cli.connect_agent(create_run(checkout, "deploy", {}))[1].identity == "other-branch"
    assert seen == ["retrieve"]
