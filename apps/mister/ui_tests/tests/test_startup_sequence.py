"""Startup presentation ordering across warm, cold, and failure paths."""

from __future__ import annotations

import os
import shlex

import pytest

from apps.mister.ui_tests.agent_input import StartupPresentationTrace
from apps.mister.ui_tests.driver import (
    DriverConfig,
    MagiKDriver,
    environment_for_application,
)

DISPLAY_CONTRACTS = {
    "hdmi-1080p": ("hdmi", "hdmi-1920x1080p60"),
    "crt-240p": ("crt-240p60", "crt-240p60"),
}


@pytest.mark.parametrize(
    ("mode", "display", "expected_kinds"),
    (
        ("warm-ready", "hdmi-1080p", ("launcher",)),
        ("warm-ready", "crt-240p", ("launcher",)),
        ("warm-hydrating", "hdmi-1080p", ("launcher",)),
        ("cold-delayed", "hdmi-1080p", ("particle-intro", "launcher")),
        ("cold-intro-failure", "hdmi-1080p", ("catalog-progress", "launcher")),
    ),
)
def test_startup_presentation_sequence(
    mode: str,
    display: str,
    expected_kinds: tuple[str, ...],
) -> None:
    command_text = os.environ.get("MISTER_UI_TEST_COMMAND")
    if not command_text:
        pytest.skip("set MISTER_UI_TEST_COMMAND for attended device UI tests")
    output_route, display_mode = DISPLAY_CONTRACTS[display]
    environment = environment_for_application()
    environment.update(
        {
            "MISTER_UI_TEST_DISPLAY": display,
            "MISTER_UI_TEST_ORIENTATION": "normal",
            "MISTER_UI_TEST_STARTUP_MODE": mode,
            "MISTER_MAGIK_RUNTIME_SETTINGS_V1": f"schema=1&output={output_route}",
            "MISTER_MAGIK_RUNTIME_DISPLAY_V1": f"schema=1&mode={display_mode}",
        }
    )
    config = DriverConfig(
        command=tuple(shlex.split(command_text)),
        environment=environment,
        launch_timeout=float(os.environ.get("MISTER_UI_TEST_LAUNCH_TIMEOUT", "20")),
    )
    with MagiKDriver.start(config) as driver:
        trace = driver.wait_for_startup_sequence(
            lambda value: _kinds(value) == expected_kinds,
            timeout=45.0,
        )
        assert not trace.truncated
        assert trace.first_launcher_frame is not None
        assert trace.first_input_enabled_frame is not None
        assert trace.first_input_enabled_frame >= trace.first_launcher_frame
        assert all("splash" not in entry.kind for entry in trace.entries)
        launcher_position = next(
            index
            for index, entry in enumerate(trace.entries)
            if entry.kind == "launcher"
        )
        if mode == "warm-hydrating":
            assert trace.entries[launcher_position].catalog_ready
        assert all(
            not entry.input_enabled for entry in trace.entries[:launcher_position]
        )
        if mode == "cold-intro-failure":
            assert trace.intro_failure
        else:
            assert trace.intro_failure is None


def _kinds(trace: StartupPresentationTrace) -> tuple[str, ...]:
    return tuple(entry.kind for entry in trace.entries)


__all__ = ["DISPLAY_CONTRACTS"]
