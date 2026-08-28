"""Attended orchestration for device-only MagiK UI journeys."""

from __future__ import annotations

import argparse
import subprocess
from dataclasses import dataclass
from pathlib import Path
from typing import Protocol

DEFAULT_FIXTURE = "deterministic-arcade-v1"
DEFAULT_TIMEOUT_SECONDS = 120


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

    def run(self, case: UiCase) -> UiCaseResult:
        command = [
            str(self._command),
            "device",
            "launcher",
            "ui-test",
            "--case",
            case.name,
            "--fixture",
            case.fixture,
            "--timeout-secs",
            str(case.timeout_seconds),
            "--attended",
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
                f"device UI case {case.name!r} failed ({completed.returncode}): {output}"
            )
        return UiCaseResult(case, output)


def run_cases(cases: list[UiCase], bridge: AgentBridge) -> list[UiCaseResult]:
    """Run cases in declared order; no retries hide flaky behavior."""

    return [bridge.run(case) for case in cases]


def _parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("case", nargs="+", help="case names to run")
    parser.add_argument("--fixture", default=DEFAULT_FIXTURE)
    parser.add_argument("--timeout-secs", type=int, default=DEFAULT_TIMEOUT_SECONDS)
    parser.add_argument("--repository", type=Path, default=Path(__file__).parents[3])
    parser.add_argument("--attended", action="store_true", required=True)
    return parser.parse_args()


def main() -> int:
    arguments = _parse_args()
    if arguments.timeout_secs < 1 or arguments.timeout_secs > 600:
        raise SystemExit("--timeout-secs must be between 1 and 600")
    cases = [
        UiCase(name, arguments.fixture, arguments.timeout_secs)
        for name in arguments.case
    ]
    results = run_cases(cases, ScriptAgentBridge(arguments.repository))
    for result in results:
        if result.output:
            print(result.output)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())


__all__ = ["AgentBridge", "ScriptAgentBridge", "UiCase", "UiCaseResult", "run_cases"]
