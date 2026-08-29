"""Typed MagiK UI driver combining Slint inspection with agent input."""

from __future__ import annotations

import contextlib
import tempfile
from collections.abc import Iterator, Mapping
from dataclasses import dataclass
from pathlib import Path

from .agent_input import AgentInput, Button, Key
from .input_correlation import CorrelatedInput, InputCorrelation
from .slint_adapter import SlintApplication, load_application_factory, require_window


@dataclass(frozen=True)
class DriverConfig:
    """Launch parameters for one device UI process."""

    command: tuple[Path | str, ...]
    environment: Mapping[str, str]
    launch_timeout: float = 20.0


def environment_for_application() -> dict[str, str]:
    """Return host bridge variables without the private Slint package token."""

    import os

    environment = dict(os.environ)
    return {
        key: value
        for key, value in environment.items()
        if key.startswith(("MISTER_", "SLINT_", "UV_"))
        and key
        not in {
            "SLINT_TESTING_TOKEN",
            "UV_INDEX_SLINT_PRIVATE_PASSWORD",
            "MISTER_UI_TEST_SSH_DESTINATION",
        }
    }


class MagiKDriver:
    """Window oracle and physical-input driver for one application session."""

    def __init__(
        self,
        application: SlintApplication,
        inputs: AgentInput,
    ) -> None:
        self.application = application
        self.window = require_window(application)
        self.inputs = InputCorrelation(self.window.root_element, inputs)

    @classmethod
    @contextlib.contextmanager
    def start(cls, config: DriverConfig) -> Iterator[MagiKDriver]:
        if not config.command:
            raise ValueError("MagiK UI driver requires a non-empty launch command")
        factory = load_application_factory()
        arguments = [str(argument) for argument in config.command]
        with tempfile.TemporaryDirectory(prefix="mister-magik-ui-test-") as directory:
            control_socket = Path(directory) / "control.sock"
            arguments.extend(["--control-socket", str(control_socket)])
            application = factory(
                arguments,
                env=config.environment,
                launch_timeout=config.launch_timeout,
            )
            with application:
                yield cls(application, AgentInput(control_socket))

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
