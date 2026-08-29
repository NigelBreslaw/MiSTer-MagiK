"""Host-only contracts for the attended UI-test suite orchestration."""

from __future__ import annotations

import sys
from pathlib import Path
from unittest.mock import patch

from apps.mister.ui_tests import suite


def test_suite_preflights_host_client_before_build_and_pytest() -> None:
    events: list[str] = []

    with (
        patch.object(
            suite, "load_application_factory", lambda: events.append("client")
        ),
        patch.object(
            suite,
            "run_cases",
            lambda _cases, _bridge: events.append("build") or [],
        ),
        patch.object(
            suite,
            "run_pytest",
            lambda _cases, _repository: events.append("pytest") or "",
        ),
        patch.object(
            sys,
            "argv",
            ["suite", "smoke", "--repository", str(Path.cwd()), "--attended"],
        ),
    ):
        assert suite.main() == 0

    assert events == ["client", "build", "pytest"]


def test_suite_host_client_errors_are_not_hidden() -> None:
    def fail() -> None:
        raise RuntimeError("slint-testing==0.3 is required")

    with (
        patch.object(suite, "load_application_factory", fail),
        patch.object(sys, "argv", ["suite", "smoke", "--attended"]),
    ):
        try:
            suite.main()
        except RuntimeError as error:
            assert str(error) == "slint-testing==0.3 is required"
        else:
            raise AssertionError("missing host client should fail before the build")
