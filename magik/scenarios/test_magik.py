"""Small real-app checks; the scenario is also its benchmark workload."""

import time
import json
import os
import uuid
from pathlib import Path

import pytest
from actions import (
    launcher_smoke,
    launcher_idle,
    launcher_navigation,
    launcher_catalog,
    launcher_setting,
    validate_development_paths,
)
from magik.results import append_event
from catalog_equivalence import (
    catalog_identity,
    assert_catalog_equivalent,
    restore_application,
)


@pytest.mark.skipif(
    os.environ.get("MISTER_MAGIK2_CATALOG_EQUIVALENCE") != "1",
    reason="fresh catalog equivalence requires explicit opt-in",
)
def test_catalog_equivalence(magik_run):
    """Build from today's installed sources into a new isolated catalog root."""
    from magik.apps import application
    from magik.cli import connect_agent, ensure_application, CHECK_AGENT_CAPABILITIES
    from magik.client import AgentError
    from magik.results import retain_diagnostics

    agent, status = connect_agent(
        magik_run,
        CHECK_AGENT_CAPABILITIES
        | {"artifacts-v1"}
        | application("magik").agent_capabilities,
    )
    ensure_application(agent, status, magik_run, "magik")
    session_id = f"catalog-equivalence-{uuid.uuid4().hex}"
    try:
        agent._successful(
            "start",
            {
                "artifact": "magik",
                "restart": True,
                "expected_sha256": agent.expected_sha256,
                "profile_id": session_id,
            },
        )
        deadline = time.monotonic() + 900
        while True:
            try:
                raw = agent.read_profile_artifact(session_id, "catalog.json")
                break
            except AgentError as error:
                if not str(error).startswith("artifact-unavailable"):
                    raise
                if time.monotonic() >= deadline:
                    raise AssertionError(
                        "fresh catalog did not finish within 900 seconds"
                    ) from error
                time.sleep(2)
        (magik_run / "catalog.json").write_bytes(raw)
        result = json.loads(raw)
        catalog_identity(result)
        assert result["artifact_sha256"] == agent.expected_sha256
        baseline = os.environ.get("MISTER_MAGIK2_CATALOG_BASELINE")
        if baseline:
            before = json.loads(Path(baseline).read_text())
            assert_catalog_equivalent(before, result)
        append_event(
            magik_run,
            {
                "phase": "catalog-equivalence",
                "outcome": "passed",
                "artifact_sha256": agent.expected_sha256,
                "session_id": session_id,
                "systems": len(result["systems"]),
                "games": sum(system["games"] for system in result["systems"]),
                "baseline": baseline,
            },
        )
    finally:
        restore_application(agent, magik_run, retain_diagnostics)


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


@pytest.mark.magik_profile
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


@pytest.mark.magik_profile
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
