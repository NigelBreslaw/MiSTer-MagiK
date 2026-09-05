"""Consumer scenarios: the same pytest cases assert behavior and retain timings."""

import pytest
from actions import smoke, motion
from magik2.results import append_event


def test_smoke(probe_session):
    application, agent, run, profile_id = probe_session
    result = smoke(application, run / "smoke.png", agent.expected_sha256)
    append_event(run, {"phase": "smoke", "outcome": "passed", **result})


@pytest.mark.parametrize("repetition", range(5))
def test_motion(probe_session, repetition):
    application, agent, run, profile_id = probe_session
    result = motion(application, agent)
    append_event(
        run,
        {"phase": "motion", "outcome": "measured", "repetition": repetition, **result},
    )


@pytest.mark.magik2_profile
def test_motion_profile(probe_session):
    application, agent, run, profile_id = probe_session
    result = motion(application, agent, instrumented=True)
    append_event(
        run,
        {"phase": "motion", "outcome": "measured", "profile_id": profile_id, **result},
    )
