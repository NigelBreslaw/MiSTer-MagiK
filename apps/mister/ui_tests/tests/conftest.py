"""Fixtures for the attended, device-only MagiK UI suite."""

from __future__ import annotations

import os
import shlex
from collections.abc import Iterator

import pytest

from apps.mister.ui_tests.driver import (
    DriverConfig,
    MagiKDriver,
    environment_for_application,
)


def pytest_sessionfinish(session: pytest.Session, exitstatus: int) -> None:
    """Make skipped tests fatal when invoked by the attended suite."""

    if os.environ.get("MISTER_UI_TEST_FAIL_ON_SKIP") != "1":
        return
    terminal = session.config.pluginmanager.get_plugin("terminalreporter")
    skipped = terminal.stats.get("skipped", []) if terminal is not None else []
    if skipped:
        session.exitstatus = pytest.ExitCode.TESTS_FAILED


@pytest.fixture
def magik() -> Iterator[MagiKDriver]:
    """Launch the configured device binary for one isolated test case."""

    command_text = os.environ.get("MISTER_UI_TEST_COMMAND")
    if not command_text:
        pytest.skip("set MISTER_UI_TEST_COMMAND for attended device UI tests")
    command = tuple(shlex.split(command_text))
    environment = environment_for_application()
    environment.setdefault("MISTER_UI_TEST_FIXTURE", "deterministic-arcade-v1")
    config = DriverConfig(
        command=command,
        environment=environment,
        launch_timeout=float(os.environ.get("MISTER_UI_TEST_LAUNCH_TIMEOUT", "20")),
    )
    with MagiKDriver.start(config) as driver:
        yield driver


@pytest.fixture
def controller() -> Iterator[MagiKDriver]:
    """Launch the launcher controller screen with logical test input."""

    command_text = os.environ.get("MISTER_UI_TEST_COMMAND")
    if not command_text:
        pytest.skip("set MISTER_UI_TEST_COMMAND for attended device UI tests")
    environment = environment_for_application()
    environment.setdefault("MISTER_UI_TEST_FIXTURE", "deterministic-arcade-v1")
    environment["MISTER_UI_TEST_FEATURE"] = "controller"
    config = DriverConfig(
        command=tuple(shlex.split(command_text)),
        environment=environment,
        launch_timeout=float(os.environ.get("MISTER_UI_TEST_LAUNCH_TIMEOUT", "20")),
    )
    with MagiKDriver.start(config) as driver:
        yield driver
