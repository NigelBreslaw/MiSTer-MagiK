"""Consumer scenarios: the same pytest cases assert behavior and retain timings."""

import pytest
from actions import launcher_motion, motion, smoke
from magik2.results import append_event


def test_smoke(application_session):
    application, agent, run, profile_id = application_session
    result = smoke(application, agent, run / "smoke.png", agent.expected_sha256)
    append_event(run, {"phase": "smoke", "outcome": "passed", **result})


@pytest.mark.parametrize("repetition", range(2))
def test_motion(application_session, repetition):
    application, agent, run, profile_id = application_session
    result = motion(application, agent)
    append_event(
        run,
        {"phase": "motion", "outcome": "measured", "repetition": repetition, **result},
    )


@pytest.mark.magik2_profile
def test_motion_profile(application_session):
    application, agent, run, profile_id = application_session
    result = motion(application, agent, instrumented=True)
    append_event(
        run,
        {"phase": "motion", "outcome": "measured", "profile_id": profile_id, **result},
    )


@pytest.mark.parametrize("direction", ["right", "left"])
def test_launcher_motion(application_session, direction):
    application, agent, run, profile_id = application_session
    result = launcher_motion(application, agent, direction)
    append_event(
        run,
        {"phase": "launcher-motion", "outcome": "measured", **result},
    )
