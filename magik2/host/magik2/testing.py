"""Small Slint system-testing adapter for the one 2.0 probe application."""

from __future__ import annotations

import contextlib
import importlib
import os
import sys
from collections.abc import Iterator
from pathlib import Path
from typing import Any

from .client import NativeAgent


def _application_factory() -> Any:
    try:
        module = importlib.import_module("slint_testing")
    except ModuleNotFoundError as error:
        raise RuntimeError("slint-testing==0.3 is required for magik2 check") from error
    factory = getattr(module, "Application", None)
    if not callable(factory):
        raise RuntimeError("slint-testing exposes no Application client")
    return factory


@contextlib.contextmanager
def fresh_session(agent: NativeAgent, timeout: float = 20, profile_id: str | None = None) -> Iterator[Any]:
    """Attach a fresh Slint test session over the native agent only."""
    environment = {
        key: value
        for key, value in os.environ.items()
        if key in {"PATH", "LANG", "LC_ALL", "TMPDIR", "PYTHONPATH", "VIRTUAL_ENV"}
    }
    environment.update(
        {
            "MISTER_IP": agent.host,
            "MISTER_MAGIK2_PORT": str(agent.port),
            "MISTER_MAGIK2_TOKEN": agent.token,
        }
    )
    factory = _application_factory()
    bridge = [sys.executable, "-m", "magik2.test_bridge"]
    if profile_id is not None:
        bridge.extend(["--profile-id", profile_id])
    with factory(
        bridge,
        env=environment,
        launch_timeout=timeout,
    ) as application:
        yield application


def one_element(application: Any, label: str) -> Any:
    from slint_testing import AccessibleRole

    window = application.first_window
    if window is None:
        raise AssertionError("probe exposed no Slint window")
    role = AccessibleRole.Text if label in {"build-label", "counter", "details-panel", "motion-state"} else AccessibleRole.Button
    matches = [
        element
        for element in window.root_element.query_descendants().match_accessible_role(role).find_all()
        if element.accessible_label == label
    ]
    if len(matches) != 1:
        raise AssertionError(f"expected one element for {label!r}, got {len(matches)}")
    return matches[0]


def screenshot(application: Any, destination: Path) -> None:
    window = application.first_window
    if window is None:
        raise AssertionError("probe exposed no Slint window")
    destination.write_bytes(window.grab_window_as_png())
