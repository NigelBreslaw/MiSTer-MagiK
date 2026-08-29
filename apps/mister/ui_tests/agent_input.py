"""Logical keyboard and joystick actions sent to the local MagiK bridge."""

from __future__ import annotations

import json
import socket
from enum import StrEnum
from pathlib import Path


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

    def key(self, key: Key) -> None:
        self._request({"kind": "tap", "key": key.value})

    def button(self, button: Button) -> None:
        self._request({"kind": "tap", "button": button.value})

    def hat(self, horizontal: int, vertical: int) -> None:
        if horizontal not in (-1, 0, 1) or vertical not in (-1, 0, 1):
            raise ValueError("logical UI-test hat values must be -1, 0, or 1")
        self._request({"kind": "hat", "horizontal": horizontal, "vertical": vertical})

    def _request(self, payload: dict[str, object]) -> None:
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


__all__ = ["AgentInput", "Button", "Key"]
