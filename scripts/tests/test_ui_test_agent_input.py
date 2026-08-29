"""Host-only contracts for the agent-backed UI-test input client."""

from __future__ import annotations

import json
from pathlib import Path
from typing import Self
from unittest.mock import patch

from apps.mister.ui_tests.agent_input import AgentInput, Button, Key
from apps.mister.ui_tests.driver import environment_for_application


def test_agent_input_sends_closed_logical_actions(tmp_path: Path) -> None:
    requests: list[dict[str, object]] = []

    class FakeFile:
        def readline(self) -> bytes:
            return b'{"ok":true}\n'

    class FakeSocket:
        def __enter__(self) -> Self:
            return self

        def __exit__(self, *_args: object) -> None:
            return None

        def settimeout(self, _timeout: float) -> None:
            return None

        def connect(self, _path: str) -> None:
            return None

        def sendall(self, value: bytes) -> None:
            requests.append(json.loads(value))

        def makefile(self, _mode: str) -> FakeFile:
            return FakeFile()

    with patch(
        "apps.mister.ui_tests.agent_input.socket.socket", return_value=FakeSocket()
    ):
        inputs = AgentInput(tmp_path / "input.sock")
        inputs.key(Key.F12)
        inputs.button(Button.A)
        inputs.hat(1, 0)

    assert [request["schema"] for request in requests] == [
        "mister-magik-ui-test-input-v1"
    ] * 3
    assert requests[0]["key"] == "f12"
    assert requests[1]["button"] == "a"
    assert requests[2]["horizontal"] == 1


def test_application_environment_excludes_private_testing_credentials(
    monkeypatch,
) -> None:
    monkeypatch.setenv("SLINT_TESTING_TOKEN", "private")
    monkeypatch.setenv("SLINT_TEST_TOKEN", "private")
    monkeypatch.setenv("UV_INDEX_SLINT_PRIVATE_PASSWORD", "private")
    monkeypatch.setenv("MISTER_AGENT_TOKEN", "agent")
    monkeypatch.setenv("MISTER_DEVICE_ID", "device")
    monkeypatch.setenv("HOME", "/operator-home")
    monkeypatch.setenv("PATH", "/operator-bin")

    environment = environment_for_application()

    assert "SLINT_TESTING_TOKEN" not in environment
    assert "SLINT_TEST_TOKEN" not in environment
    assert "UV_INDEX_SLINT_PRIVATE_PASSWORD" not in environment
    assert environment["MISTER_AGENT_TOKEN"] == "agent"
    assert environment["MISTER_DEVICE_ID"] == "device"
    assert environment["HOME"] == "/operator-home"
    assert environment["PATH"] == "/operator-bin"
