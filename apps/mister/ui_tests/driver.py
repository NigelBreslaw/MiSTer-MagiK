"""Typed MagiK UI driver combining Slint inspection with agent input."""

from __future__ import annotations

import contextlib
import tempfile
import time
from collections.abc import Callable, Iterator, Mapping
from dataclasses import dataclass
from pathlib import Path

from .agent_input import (
    AgentInput,
    Button,
    Key,
    StartupPresentationTrace,
    UiSemanticState,
)
from .input_correlation import CorrelatedInput, InputCorrelation
from .slint_adapter import SlintApplication, load_application_factory, require_window


@dataclass(frozen=True)
class DriverConfig:
    """Launch parameters for one device UI process."""

    command: tuple[Path | str, ...]
    environment: Mapping[str, str]
    launch_timeout: float = 20.0


def environment_for_application() -> dict[str, str]:
    """Return bridge variables and safe process paths without private tokens."""

    import os

    environment = dict(os.environ)
    return {
        key: value
        for key, value in environment.items()
        if (
            key.startswith(("MISTER_", "SLINT_", "UV_"))
            or key in {"HOME", "PATH", "TMPDIR", "XDG_CONFIG_HOME", "LANG", "LC_ALL"}
        )
        and (
            key == "MISTER_AGENT_TOKEN"
            or not any(
                marker in key.upper()
                for marker in ("TOKEN", "PASSWORD", "SECRET", "CREDENTIAL")
            )
        )
    }


class MagiKDriver:
    """Window oracle and logical-input driver for one application session."""

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

    def keep_alive(self) -> None:
        """Renew the device automation lease during long read-only queries."""

        self.inputs.keep_alive()

    def wait_for_semantic(
        self,
        predicate: Callable[[UiSemanticState], bool],
        timeout: float = 2.0,
    ) -> UiSemanticState:
        """Wait for a presented runtime snapshot satisfying ``predicate``."""

        deadline = time.monotonic() + timeout
        while True:
            snapshot = self.inputs.snapshot()
            if (
                snapshot.state_revision > 0
                and snapshot.presented_state_revision >= snapshot.state_revision
                and predicate(snapshot.semantic)
            ):
                return snapshot.semantic
            if time.monotonic() >= deadline:
                raise AssertionError(
                    "runtime semantic state did not satisfy the expected profile "
                    f"within {timeout}s: {snapshot.semantic!r}"
                )
            time.sleep(0.02)

    def wait_for_startup_sequence(
        self,
        predicate: Callable[[StartupPresentationTrace], bool],
        timeout: float = 2.0,
    ) -> StartupPresentationTrace:
        """Wait for a startup presentation trace satisfying ``predicate``."""

        deadline = time.monotonic() + timeout
        while True:
            snapshot = self.inputs.snapshot()
            trace = snapshot.startup_trace
            if predicate(trace):
                return trace
            if time.monotonic() >= deadline:
                raise AssertionError(
                    "startup presentation trace did not satisfy the expected profile "
                    f"within {timeout}s: {trace!r}"
                )
            time.sleep(0.02)


__all__ = ["DriverConfig", "MagiKDriver", "environment_for_application"]
