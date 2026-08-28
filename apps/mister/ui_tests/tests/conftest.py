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


@pytest.fixture
def magik() -> Iterator[MagiKDriver]:
    """Launch the configured device binary for one isolated test case."""

    command_text = os.environ.get("MISTER_UI_TEST_COMMAND")
    if not command_text:
        pytest.skip("set MISTER_UI_TEST_COMMAND for attended device UI tests")
    command = tuple(shlex.split(command_text))
    environment = environment_for_application()
    environment.setdefault(
        "MISTER_UI_TEST_FIXTURE", "deterministic-arcade-v1"
    )
    config = DriverConfig(
        command=command,
        environment=environment,
        ssh_destination=os.environ.get("MISTER_UI_TEST_SSH_DESTINATION"),
        launch_timeout=float(os.environ.get("MISTER_UI_TEST_LAUNCH_TIMEOUT", "20")),
    )
    with MagiKDriver.start(config) as driver:
        yield driver


@pytest.fixture
def controller() -> Iterator[MagiKDriver]:
    """Launch the dedicated controller-test scene when configured."""

    command_text = os.environ.get("MISTER_UI_TEST_CONTROLLER_COMMAND")
    if not command_text:
        pytest.skip("set MISTER_UI_TEST_CONTROLLER_COMMAND for controller UI tests")
    config = DriverConfig(
        command=tuple(shlex.split(command_text)),
        environment=environment_for_application(),
        ssh_destination=os.environ.get("MISTER_UI_TEST_SSH_DESTINATION"),
        launch_timeout=float(os.environ.get("MISTER_UI_TEST_LAUNCH_TIMEOUT", "20")),
    )
    with MagiKDriver.start(config) as driver:
        yield driver
