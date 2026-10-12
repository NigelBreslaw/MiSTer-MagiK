"""Exercise actual pytest fixture scopes without building or contacting a device."""

from pathlib import Path

import pytest

pytest_plugins = ["pytester"]


@pytest.mark.parametrize("installed", [False, True])
def test_journeys_prepare_once_and_reuse_the_pinned_binary(
    pytester, monkeypatch, installed
):
    monkeypatch.setenv("PYTHONPATH", str(Path(__file__).resolve().parents[1]))
    pytester.makeconftest(
        """
from contextlib import contextmanager
from types import SimpleNamespace
from unittest.mock import Mock
import pytest
from magik import cli, scenario_runner

pytest_plugins = ["magik.scenario_runner"]

@pytest.fixture(scope="session", autouse=True)
def device():
    patch = pytest.MonkeyPatch()
    sha = "a" * 64
    status = SimpleNamespace(fields={
        "running": True, "ready": True, "artifact": "magik", "running_sha256": sha
    })
    agent = Mock(expected_sha256=None)
    agent.metrics.return_value = {"context": {
        "card_sampler": "current", "card_quantiser": "existing-bayer"
    }}
    prepare = Mock(side_effect=lambda *args: setattr(agent, "expected_sha256", sha))
    connect = Mock(return_value=(agent, status))
    leases = []
    @contextmanager
    def fresh_session(actual, **kwargs):
        assert actual.expected_sha256 == sha
        lease = object()
        leases.append(lease)
        yield lease
    patch.setattr(cli, "connect_agent", connect)
    patch.setattr(cli, "ensure_application", prepare)
    patch.setattr(scenario_runner, "fresh_session", fresh_session)
    yield SimpleNamespace(prepare=prepare, agent=agent, leases=leases)
    assert connect.call_count == 1
    assert len(leases) == 5 and len(set(leases)) == 5
    assert agent.start.call_count == 5
    assert all(call.kwargs == {"expected_sha256": sha} for call in agent.start.call_args_list)
    patch.undo()
"""
    )
    pytester.makepyfile(
        test_magik="""
import pytest

@pytest.mark.parametrize("repetition", range(4))
def test_journey(journey_application_session, device, repetition, request):
    application, agent, run, profile = journey_application_session
    assert application is device.leases[-1]
    assert agent.expected_sha256 == "a" * 64
    assert device.prepare.call_count == (0 if request.config.getoption("--magik-installed-sha256") else 1)

def test_shared_session(application_session, device):
    assert application_session[0] is device.leases[-1]
"""
    )
    options = ["--magik-device", "-q"]
    if installed:
        options += ["--magik-installed-sha256", "a" * 64]
    result = pytester.runpytest_subprocess(*options)
    result.assert_outcomes(passed=5)
