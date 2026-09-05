"""Small real-app checks; the scenario is also its benchmark workload."""

import pytest
from actions import launcher_smoke, launcher_idle
from magik2.results import append_event


def test_smoke(application_session):
    app, agent, run, _ = application_session
    result = launcher_smoke(app, run / "smoke.png", agent.expected_sha256)
    append_event(run, {"phase": "smoke", "outcome": "passed", **result})


@pytest.mark.parametrize("repetition", range(2))
def test_idle(application_session, repetition):
    app, agent, run, _ = application_session
    result = launcher_idle(app, agent)
    append_event(
        run,
        {"phase": "idle", "outcome": "measured", "repetition": repetition, **result},
    )


@pytest.mark.magik2_profile
def test_idle_profile(application_session):
    app, agent, run, profile_id = application_session
    result = launcher_idle(app, agent, instrumented=True)
    append_event(
        run,
        {"phase": "idle", "outcome": "measured", "profile_id": profile_id, **result},
    )
