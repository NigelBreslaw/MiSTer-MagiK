"""Host-only contracts for the agent-backed UI-test input client."""

from __future__ import annotations

import json
from pathlib import Path
from types import SimpleNamespace
from typing import Self, cast
from unittest.mock import patch

from apps.mister.ui_tests.agent_input import (
    AgentInput,
    Button,
    Key,
    UiSemanticState,
    UiTestSnapshot,
)
from apps.mister.ui_tests.driver import MagiKDriver, environment_for_application


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


def test_agent_input_parses_presented_runtime_snapshot(tmp_path: Path) -> None:
    class FakeFile:
        def readline(self) -> bytes:
            return (
                b'{"ok":true,"snapshot":{"state_revision":4,'
                b'"presented_state_revision":4,"semantic":{'
                b'"screen_orientation":"Normal","output_route":"hdmi",'
                b'"output_width":1280,"output_height":720,"render_width":1280,'
                b'"render_height":720,"effective_view":"home"}}}\n'
            )

    class FakeSocket:
        def __enter__(self) -> Self:
            return self

        def __exit__(self, *_args: object) -> None:
            return None

        def settimeout(self, _timeout: float) -> None:
            return None

        def connect(self, _path: str) -> None:
            return None

        def sendall(self, _value: bytes) -> None:
            return None

        def makefile(self, _mode: str) -> FakeFile:
            return FakeFile()

    with patch(
        "apps.mister.ui_tests.agent_input.socket.socket", return_value=FakeSocket()
    ):
        snapshot = AgentInput(tmp_path / "input.sock").snapshot()

    assert snapshot.presented_state_revision == snapshot.state_revision == 4
    assert snapshot.semantic.output_route == "hdmi"
    assert (snapshot.semantic.output_width, snapshot.semantic.output_height) == (
        1280,
        720,
    )


def test_agent_input_rejects_malformed_runtime_snapshot(tmp_path: Path) -> None:
    class FakeFile:
        def readline(self) -> bytes:
            return b'{"ok":true,"snapshot":{"semantic":{}}}\n'

    class FakeSocket:
        def __enter__(self) -> Self:
            return self

        def __exit__(self, *_args: object) -> None:
            return None

        def settimeout(self, _timeout: float) -> None:
            return None

        def connect(self, _path: str) -> None:
            return None

        def sendall(self, _value: bytes) -> None:
            return None

        def makefile(self, _mode: str) -> FakeFile:
            return FakeFile()

    with patch(
        "apps.mister.ui_tests.agent_input.socket.socket", return_value=FakeSocket()
    ):
        try:
            AgentInput(tmp_path / "input.sock").snapshot()
        except RuntimeError as error:
            assert "state_revision" in str(error)
        else:
            raise AssertionError("malformed runtime snapshot should fail")


def test_driver_waits_for_a_presented_semantic_snapshot() -> None:
    state = UiSemanticState(
        screen_orientation="Normal",
        output_route="hdmi",
        output_width=1280,
        output_height=720,
        render_width=1280,
        render_height=720,
        effective_view="home",
    )

    class FakeInputs:
        def __init__(self) -> None:
            self.snapshots = iter(
                [
                    UiTestSnapshot(2, 1, state),
                    UiTestSnapshot(2, 2, state),
                ]
            )

        def snapshot(self) -> UiTestSnapshot:
            return next(self.snapshots)

    driver = cast(MagiKDriver, SimpleNamespace(inputs=FakeInputs()))
    assert (
        driver.wait_for_semantic(lambda value: value.effective_view == "home") == state
    )


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
