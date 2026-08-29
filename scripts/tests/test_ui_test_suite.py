"""Host-only contracts for the attended UI-test suite orchestration."""

from __future__ import annotations

import subprocess
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


def test_complete_case_list_covers_every_non_smoke_target() -> None:
    assert set(suite.COMPLETE_CASES) == set(suite.CASE_TARGETS) - {"smoke"}
    assert suite.COMPLETE_CASES[0] == "startup-home"
    assert suite.COMPLETE_CASES[-1] == "profile-matrix"


def test_run_pytest_sets_fail_on_skip_and_reports_elapsed_case(
    tmp_path: Path,
) -> None:
    calls: list[dict[str, object]] = []

    def fake_run(command, *, check, cwd, env, capture_output, text):
        calls.append({"command": command, "cwd": cwd, "env": env})
        return subprocess.CompletedProcess(
            command,
            0,
            stdout="1 passed in 0.01s",
            stderr="",
        )

    with patch.object(suite.subprocess, "run", fake_run):
        output = suite.run_pytest([suite.UiCase("controller")], tmp_path)

    assert calls[0]["cwd"] == tmp_path
    environment = calls[0]["env"]
    assert isinstance(environment, dict)
    assert environment["MISTER_UI_TEST_FAIL_ON_SKIP"] == "1"
    assert "[controller elapsed=" in output
