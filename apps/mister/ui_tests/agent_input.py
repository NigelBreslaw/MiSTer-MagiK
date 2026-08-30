"""Logical keyboard and joystick actions sent to the local MagiK bridge."""

from __future__ import annotations

import json
import socket
import time
from dataclasses import dataclass
from enum import StrEnum
from pathlib import Path
from typing import cast


@dataclass(frozen=True)
class UiSemanticState:
    """Resolved runtime state published by the authenticated UI bridge."""

    screen_orientation: str
    output_route: str
    output_width: int
    output_height: int
    render_width: int
    render_height: int
    effective_view: str


@dataclass(frozen=True)
class StartupPresentation:
    """One physically confirmed startup content run."""

    kind: str
    frame: int
    latch: int
    elapsed_ms: int
    catalog_ready: bool
    input_enabled: bool


@dataclass(frozen=True)
class StartupPresentationTrace:
    """Bounded startup presentation history retained before bridge attach."""

    entries: tuple[StartupPresentation, ...]
    first_launcher_frame: int | None
    first_input_enabled_frame: int | None
    intro_failure: str | None
    truncated: bool


@dataclass(frozen=True)
class UiTestSnapshot:
    """Presented semantic state and its monotonic revisions."""

    state_revision: int
    presented_state_revision: int
    semantic: UiSemanticState
    startup_trace: StartupPresentationTrace


def _required_mapping(value: object, name: str) -> dict[str, object]:
    if not isinstance(value, dict):
        raise TypeError(f"MagiK UI-test snapshot field {name!r} is not an object")
    return cast(dict[str, object], value)


def _required_text(value: object, name: str) -> str:
    if not isinstance(value, str):
        raise TypeError(f"MagiK UI-test snapshot field {name!r} is not text")
    return value


def _required_integer(value: object, name: str) -> int:
    if type(value) is not int:
        raise RuntimeError(f"MagiK UI-test snapshot field {name!r} is not an integer")
    return value


def _required_boolean(value: object, name: str) -> bool:
    if type(value) is not bool:
        raise RuntimeError(f"MagiK UI-test snapshot field {name!r} is not a boolean")
    return value


class Key(StrEnum):
    """Keyboard actions supported by the logical launcher input boundary."""

    ESCAPE = "escape"
    ENTER = "enter"
    SPACE = "space"
    BACKSPACE = "backspace"
    HOME = "home"
    UP = "up"
    LEFT = "left"
    RIGHT = "right"
    DOWN = "down"
    F12 = "f12"


class Button(StrEnum):
    """Buttons supported by the logical launcher input boundary."""

    A = "a"
    B = "b"
    X = "x"
    Y = "y"
    HOME = "home"


class AgentInput:
    """Request/response client for one bridge control socket."""

    def __init__(self, socket_path: Path, timeout: float = 5.0) -> None:
        self._socket_path = socket_path
        self._timeout = timeout
        self._last_activity = 0.0

    def key(self, key: Key) -> None:
        self._request({"kind": "tap", "key": key.value})

    def button(self, button: Button) -> None:
        self._request({"kind": "tap", "button": button.value})

    def hat(self, horizontal: int, vertical: int) -> None:
        if horizontal not in (-1, 0, 1) or vertical not in (-1, 0, 1):
            raise ValueError("logical UI-test hat values must be -1, 0, or 1")
        self._request({"kind": "hat", "horizontal": horizontal, "vertical": vertical})

    def snapshot(self) -> UiTestSnapshot:
        response = self._request({"kind": "snapshot"})
        snapshot = _required_mapping(response.get("snapshot"), "snapshot")
        semantic = _required_mapping(snapshot.get("semantic"), "semantic")
        startup_trace = _required_mapping(snapshot.get("startup_trace"), "startup_trace")
        raw_entries = startup_trace.get("entries")
        if not isinstance(raw_entries, list):
            raise TypeError("MagiK UI-test snapshot field 'startup_trace.entries' is not a list")
        entries: list[StartupPresentation] = []
        for index, raw_entry in enumerate(raw_entries):
            entry = _required_mapping(raw_entry, f"startup_trace.entries[{index}]")
            entries.append(
                StartupPresentation(
                    kind=_required_text(entry.get("kind"), "startup_trace.entries.kind"),
                    frame=_required_integer(entry.get("frame"), "startup_trace.entries.frame"),
                    latch=_required_integer(entry.get("latch"), "startup_trace.entries.latch"),
                    elapsed_ms=_required_integer(
                        entry.get("elapsed_ms"), "startup_trace.entries.elapsed_ms"
                    ),
                    catalog_ready=_required_boolean(
                        entry.get("catalog_ready"), "startup_trace.entries.catalog_ready"
                    ),
                    input_enabled=_required_boolean(
                        entry.get("input_enabled"), "startup_trace.entries.input_enabled"
                    ),
                )
            )
        return UiTestSnapshot(
            state_revision=_required_integer(
                snapshot.get("state_revision"), "state_revision"
            ),
            presented_state_revision=_required_integer(
                snapshot.get("presented_state_revision"), "presented_state_revision"
            ),
            semantic=UiSemanticState(
                screen_orientation=_required_text(
                    semantic.get("screen_orientation"), "screen_orientation"
                ),
                output_route=_required_text(
                    semantic.get("output_route"), "output_route"
                ),
                output_width=_required_integer(
                    semantic.get("output_width"), "output_width"
                ),
                output_height=_required_integer(
                    semantic.get("output_height"), "output_height"
                ),
                render_width=_required_integer(
                    semantic.get("render_width"), "render_width"
                ),
                render_height=_required_integer(
                    semantic.get("render_height"), "render_height"
                ),
                effective_view=_required_text(
                    semantic.get("effective_view"), "effective_view"
                ),
            ),
            startup_trace=StartupPresentationTrace(
                entries=tuple(entries),
                first_launcher_frame=(
                    None
                    if startup_trace.get("first_launcher_frame") is None
                    else _required_integer(
                        startup_trace.get("first_launcher_frame"),
                        "startup_trace.first_launcher_frame",
                    )
                ),
                first_input_enabled_frame=(
                    None
                    if startup_trace.get("first_input_enabled_frame") is None
                    else _required_integer(
                        startup_trace.get("first_input_enabled_frame"),
                        "startup_trace.first_input_enabled_frame",
                    )
                ),
                intro_failure=(
                    None
                    if startup_trace.get("intro_failure") is None
                    else _required_text(
                        startup_trace.get("intro_failure"), "startup_trace.intro_failure"
                    )
                ),
                truncated=_required_boolean(
                    startup_trace.get("truncated"), "startup_trace.truncated"
                ),
            ),
        )

    def keep_alive(self) -> None:
        """Renew the automation lease without changing logical input state."""

        if time.monotonic() - self._last_activity < 2.0:
            return
        self._request({"kind": "snapshot"})

    def _request(self, payload: dict[str, object]) -> dict[str, object]:
        payload["schema"] = "mister-magik-ui-test-input-v1"
        try:
            with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as connection:
                connection.settimeout(self._timeout)
                connection.connect(str(self._socket_path))
                connection.sendall((json.dumps(payload) + "\n").encode())
                response = connection.makefile("rb").readline()
        except OSError as error:
            raise RuntimeError(
                f"unable to send logical UI-test input through {self._socket_path}"
            ) from error
        if not response:
            raise RuntimeError("MagiK UI-test bridge closed the input connection")
        try:
            decoded = json.loads(response)
        except json.JSONDecodeError as error:
            raise RuntimeError(
                "MagiK UI-test bridge returned invalid input JSON"
            ) from error
        if decoded.get("ok") is not True:
            raise RuntimeError(
                str(decoded.get("error", "MagiK UI-test input was rejected"))
            )
        self._last_activity = time.monotonic()
        return cast(dict[str, object], decoded)


__all__ = [
    "AgentInput",
    "Button",
    "Key",
    "StartupPresentation",
    "StartupPresentationTrace",
    "UiSemanticState",
    "UiTestSnapshot",
]
