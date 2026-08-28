"""Typed MagiK UI driver combining Slint inspection with real input devices."""

from __future__ import annotations

import contextlib
from collections.abc import Iterator, Mapping
from dataclasses import dataclass
from pathlib import Path

from .input_correlation import CorrelatedInput, InputCorrelation
from .slint_adapter import SlintApplication, load_application_factory, require_window
from .uinput_joystick import Button, VirtualJoystick
from .uinput_keyboard import Key, VirtualKeyboard


@dataclass(frozen=True)
class DriverConfig:
    """Launch parameters for one device UI process."""

    command: tuple[Path | str, ...]
    environment: Mapping[str, str]
    ssh_destination: str | None = None
    launch_timeout: float = 20.0


def environment_for_application() -> dict[str, str]:
    """Return process variables, excluding credentials for remote launches."""

    import os

    environment = dict(os.environ)
    if not environment.get("MISTER_UI_TEST_SSH_DESTINATION"):
        return environment
    return {
        key: value
        for key, value in environment.items()
        if (key.startswith("MISTER_") or key.startswith("SLINT_"))
        and not any(
            marker in key.upper()
            for marker in ("TOKEN", "PASSWORD", "SECRET", "CREDENTIAL")
        )
    }


class MagiKDriver:
    """Window oracle and physical-input driver for one application session."""

    def __init__(
        self,
        application: SlintApplication,
        keyboard: VirtualKeyboard,
        joystick: VirtualJoystick,
    ) -> None:
        self.application = application
        self.window = require_window(application)
        self.inputs = InputCorrelation(self.window.root_element, keyboard, joystick)

    @classmethod
    @contextlib.contextmanager
    def start(cls, config: DriverConfig) -> Iterator[MagiKDriver]:
        if not config.command:
            raise ValueError("MagiK UI driver requires a non-empty launch command")
        factory = load_application_factory()
        arguments = [str(argument) for argument in config.command]
        application = factory(
            arguments,
            ssh_destination=config.ssh_destination,
            env=config.environment,
            launch_timeout=config.launch_timeout,
        )
        with application:
            with VirtualKeyboard() as keyboard, VirtualJoystick() as joystick:
                yield cls(application, keyboard, joystick)

    def key(self, action: str, key: Key, timeout: float = 2.0) -> CorrelatedInput:
        return self.inputs.key(action, key, timeout)

    def button(
        self, action: str, button: Button, timeout: float = 2.0
    ) -> CorrelatedInput:
        return self.inputs.button(action, button, timeout)

    def hat(
        self,
        action: str,
        horizontal: int,
        vertical: int,
        timeout: float = 2.0,
    ) -> CorrelatedInput:
        return self.inputs.hat(action, horizontal, vertical, timeout)

    def screenshot(self, path: Path) -> None:
        path.write_bytes(self.window.grab_window_as_png())


__all__ = ["DriverConfig", "MagiKDriver", "environment_for_application"]
