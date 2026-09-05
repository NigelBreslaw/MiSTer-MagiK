from __future__ import annotations

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
        def install_and_start(self, _binary) -> str:  # pragma: no cover - must not run
            raise AssertionError("compatible agent must be retained")

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
        def install_and_start(self, _binary) -> str:
            installs.append("install")
            return "replacement-token"

    monkeypatch.setattr(cli.SshBootstrap, "from_environment", lambda: Bootstrap())
    monkeypatch.setattr(cli.time, "sleep", lambda _seconds: None)
    run = create_run(tmp_path, "status", {})
    _agent, discovered = cli.connect_agent(run)

    assert installs == ["install"]
    assert discovered.supports(required)
    assert tokens == ["cached-token", "replacement-token"]


def test_absent_agent_bootstraps_and_authentication_failure_does_not(monkeypatch: pytest.MonkeyPatch, tmp_path) -> None:
    required = cli.REQUIRED_AGENT_CAPABILITIES
    configure_native(monkeypatch, tmp_path, [OSError("absent"), status("new", required)])
    installs: list[str] = []

    class Bootstrap:
        def install_and_start(self, _binary) -> str:
            installs.append("install")
            return "replacement-token"

    monkeypatch.setattr(cli.SshBootstrap, "from_environment", lambda: Bootstrap())
    monkeypatch.setattr(cli.time, "sleep", lambda _seconds: None)
    run = create_run(tmp_path, "status", {})
    assert cli.connect_agent(run)[1].supports(required)
    assert installs == ["install"]

    configure_native(monkeypatch, tmp_path / "auth", [AgentError("authentication-failed")])
    with pytest.raises(AgentError, match="authentication-failed"):
        cli.connect_agent(create_run(tmp_path / "auth", "status", {}))
    assert installs == ["install"]
