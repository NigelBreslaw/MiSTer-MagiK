"""Attended orchestration for device-only MagiK UI journeys."""

from __future__ import annotations

import argparse
import os
import shlex
import subprocess
import sys
import time
from dataclasses import dataclass
from pathlib import Path
from typing import Protocol

from .slint_adapter import load_application_factory

DEFAULT_FIXTURE = "deterministic-arcade-v1"
DEFAULT_TIMEOUT_SECONDS = 120
CASE_TARGETS = {
    "smoke": "apps/mister/ui_tests/tests/test_startup_home.py",
    "startup-home": "apps/mister/ui_tests/tests/test_startup_home.py",
    "startup-sequence": "apps/mister/ui_tests/tests/test_startup_sequence.py",
    "system-hub": "apps/mister/ui_tests/tests/test_system_hub.py",
    "arcade-navigation": "apps/mister/ui_tests/tests/test_arcade_navigation.py",
    "arcade-filters": "apps/mister/ui_tests/tests/test_arcade_filters.py",
    "settings-display": "apps/mister/ui_tests/tests/test_settings_display.py",
    "screensaver-motion": "apps/mister/ui_tests/tests/test_screensaver_motion.py",
    "about-licenses": "apps/mister/ui_tests/tests/test_about_licenses.py",
    "controller": "apps/mister/ui_tests/tests/test_controller.py",
    "menu-confirmations": "apps/mister/ui_tests/tests/test_menu_confirmations.py",
}
COMPLETE_CASES = (
    "startup-home",
    "startup-sequence",
    "system-hub",
    "arcade-navigation",
    "arcade-filters",
    "settings-display",
    "screensaver-motion",
    "about-licenses",
    "menu-confirmations",
)


@dataclass(frozen=True)
class UiCase:
    """A named journey understood by the device-side test bridge."""

    name: str
    fixture: str = DEFAULT_FIXTURE
    timeout_seconds: int = DEFAULT_TIMEOUT_SECONDS


@dataclass(frozen=True)
class UiCaseResult:
    case: UiCase
    output: str


class AgentBridge(Protocol):
    """Transport boundary used by the suite and easy to fake in unit tests."""

    def run(self, case: UiCase) -> UiCaseResult:
        """Run one case through the attended device agent."""


class ScriptAgentBridge:
    """Invoke the repository's typed agent CLI without a shell."""

    def __init__(self, repository: Path) -> None:
        self._command = repository / "scripts" / "agent"
        self._prepared = False

    def run(self, case: UiCase) -> UiCaseResult:
        if self._prepared:
            return UiCaseResult(case, "")
        command = [
            str(self._command),
            "build",
            "runtime-ui-tests",
        ]
        completed = subprocess.run(
            command,
            check=False,
            capture_output=True,
            text=True,
        )
        output = "\n".join(
            part for part in (completed.stdout, completed.stderr) if part
        ).strip()
        if completed.returncode != 0:
            raise RuntimeError(
                f"UI-test runtime build failed ({completed.returncode}): {output}"
            )
        self._prepared = True
        return UiCaseResult(case, output)


def _preflight_host_client() -> None:
    """Validate the private Slint client before the ARM build starts."""

    load_application_factory()


def run_cases(cases: list[UiCase], bridge: AgentBridge) -> list[UiCaseResult]:
    """Run cases in declared order; no retries hide flaky behavior."""

    return [bridge.run(case) for case in cases]


def run_pytest(cases: list[UiCase], repository: Path) -> str:
    """Run each mapped pytest module in its own managed agent session."""

    outputs: list[str] = []
    bridge = repository / "scripts" / "agent"
    for case in cases:
        environment = os.environ.copy()
        environment["MISTER_UI_TEST_CASE"] = case.name
        environment["MISTER_UI_TEST_FAIL_ON_SKIP"] = "1"
        environment["MISTER_UI_TEST_FIXTURE"] = case.fixture
        environment["MISTER_UI_TEST_COMMAND"] = shlex.join(
            [
                str(bridge),
                "device",
                "launcher",
                "ui-test-bridge",
                "--case",
                case.name,
                "--fixture",
                case.fixture,
                "--timeout-secs",
                str(case.timeout_seconds),
                "--attended",
            ]
        )
        started = time.monotonic()
        completed = subprocess.run(
            [sys.executable, "-m", "pytest", "-q", CASE_TARGETS[case.name]],
            check=False,
            cwd=repository,
            env=environment,
            capture_output=True,
            text=True,
        )
        output = "\n".join(
            part for part in (completed.stdout, completed.stderr) if part
        ).strip()
        if output:
            elapsed = time.monotonic() - started
            outputs.append(f"[{case.name} elapsed={elapsed:.1f}s]\n{output}")
        if completed.returncode != 0:
            raise RuntimeError(
                f"device UI pytest case {case.name!r} failed "
                f"({completed.returncode}): {output}"
            )
    return "\n".join(outputs)


def _parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "case", nargs="+", choices=sorted(CASE_TARGETS), help="case names to run"
    )
    parser.add_argument("--fixture", default=DEFAULT_FIXTURE)
    parser.add_argument("--timeout-secs", type=int, default=DEFAULT_TIMEOUT_SECONDS)
    parser.add_argument("--repository", type=Path, default=Path(__file__).parents[3])
    parser.add_argument("--attended", action="store_true", required=True)
    return parser.parse_args()


def main() -> int:
    arguments = _parse_args()
    if arguments.timeout_secs < 1 or arguments.timeout_secs > 600:
        raise SystemExit("--timeout-secs must be between 1 and 600")
    _preflight_host_client()
    cases = [
        UiCase(name, arguments.fixture, arguments.timeout_secs)
        for name in arguments.case
    ]
    results = run_cases(cases, ScriptAgentBridge(arguments.repository))
    for result in results:
        if result.output:
            print(result.output)
    pytest_output = run_pytest(cases, arguments.repository)
    if pytest_output:
        print(pytest_output)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())


__all__ = [
    "CASE_TARGETS",
    "COMPLETE_CASES",
    "AgentBridge",
    "ScriptAgentBridge",
    "UiCase",
    "UiCaseResult",
    "run_cases",
    "run_pytest",
]
