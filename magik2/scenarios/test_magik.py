"""Small real-app checks; the scenario is also its benchmark workload."""

import time

import pytest
from actions import (
    launcher_smoke,
    launcher_idle,
    launcher_navigation,
    launcher_catalog,
    launcher_setting,
    validate_development_paths,
)
from magik2.results import append_event


def test_smoke(application_session):
    app, agent, run, _ = application_session
    result = launcher_smoke(app, run / "smoke.png", agent.expected_sha256)
    result["paths"] = validate_development_paths(agent.metrics().get("context"))
    result["navigation"] = launcher_navigation(app, run / "settings.png")
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


def _journeys(application_session, repetition, selected=None):
    app, agent, run, profile_id = application_session
    paths = validate_development_paths(agent.metrics().get("context"))
    for name, action in (("catalog", launcher_catalog), ("setting", launcher_setting)):
        if selected is not None and selected != name:
            continue
        result = action(app, run / f"{name}-{repetition}.png")
        append_event(
            run,
            {
                "phase": "journeys",
                "outcome": "passed",
                "repetition": repetition,
                "profile_id": profile_id,
                "paths": paths,
                **result,
            },
        )


@pytest.mark.parametrize("journey", ["catalog", "setting"])
@pytest.mark.parametrize("repetition", range(2))
def test_journeys(application_session, repetition, journey):
    _journeys(application_session, repetition, journey)


@pytest.mark.magik2_profile
def test_journeys_profile(application_session):
    _, agent, run, profile_id = application_session
    previous = agent.metrics().get("window")
    agent._successful("measure")
    started = time.monotonic()
    time.sleep(2.3)  # Existing device-clock profiling starts after two seconds.
    _journeys(application_session, "profile")
    journey_seconds = time.monotonic() - started
    time.sleep(max(0, 12.4 - journey_seconds))
    metrics = agent.metrics()
    window = metrics.get("window")
    assert metrics.get("sha256") == agent.expected_sha256
    assert isinstance(window, dict) and window.get("instrumented") is True
    assert 10_000 <= window.get("elapsed_ms", 0) <= 11_000
    assert not window.get("evidence_error")
    if isinstance(previous, dict):
        assert window["start_ms"] > previous["end_ms"]
    assert journey_seconds < 15, (
        "journeys exceeded the 15-second allowance; discuss before rerunning"
    )
    append_event(
        run,
        {
            "phase": "journeys-profile",
            "journey_elapsed_seconds": round(journey_seconds, 3),
            "profile_scope": "ten-second device sample; not full journey coverage",
            "profile_id": profile_id,
            "outcome": "measured",
            "window": window,
        },
    )
