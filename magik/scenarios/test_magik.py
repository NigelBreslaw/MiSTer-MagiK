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
    launcher_motion,
    launcher_screensaver,
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
    result["navigation"] = launcher_navigation(app, run / "settings.png", agent=agent)
    append_event(run, {"phase": "smoke", "outcome": "passed", **result})


@pytest.mark.parametrize("repetition", range(2))
def test_idle(application_session, repetition):
    app, agent, run, _ = application_session
    result = launcher_idle(app, agent)
    append_event(
        run,
        {"phase": "idle", "outcome": "measured", "repetition": repetition, **result},
    )


def test_motion_taps_then_hold(journey_application_session):
    # Its 12.5-second window does not fit in the shared session's native
    # 60-second test lease after the other motion windows; use a fresh lease.
    app, agent, run, _ = journey_application_session
    result = launcher_motion(app, agent, taps_then_hold=True)
    append_event(
        run,
        {
            "phase": "motion",
            "outcome": "measured",
            "repetition": "six-taps-then-ten-second-hold",
            **result,
        },
    )


@pytest.mark.parametrize("repetition", [0, 1, 2, "held-eight-seconds"])
def test_motion(application_session, repetition):
    app, agent, run, _ = application_session
    result = launcher_motion(app, agent, held_direction=isinstance(repetition, str))
    append_event(
        run,
        {"phase": "motion", "outcome": "measured", "repetition": repetition, **result},
    )


def test_screensaver(application_session):
    app, agent, run, _ = application_session
    result = launcher_screensaver(app, agent)
    append_event(run, {"phase": "screensaver", "outcome": "measured", **result})


def test_motion_held(application_session):
    app, agent, run, _ = application_session
    result = launcher_motion(app, agent, held_direction=True)
    append_event(run, {"phase": "motion", "outcome": "measured", **result})


@pytest.mark.magik_profile
def test_motion_held_profile(application_session):
    app, agent, run, profile_id = application_session
    result = launcher_motion(app, agent, instrumented=True, held_direction=True)
    append_event(
        run,
        {"phase": "motion", "outcome": "measured", "profile_id": profile_id, **result},
    )


@pytest.mark.parametrize("repetition", range(3))
def test_motion_rollover(application_session, repetition):
    app, agent, run, _ = application_session
    result = launcher_motion(app, agent, align_rollover=True)
    append_event(
        run,
        {
            "phase": "motion-rollover",
            "outcome": "measured",
            "repetition": repetition,
            **result,
        },
    )


def test_motion_fallback(application_session):
    app, agent, run, _ = application_session
    result = launcher_motion(app, agent, force_fallback=True)
    append_event(run, {"phase": "motion-fallback", "outcome": "measured", **result})


@pytest.mark.magik_profile
def test_idle_profile(application_session):
    app, agent, run, profile_id = application_session
    result = launcher_idle(app, agent, instrumented=True)
    append_event(
        run,
        {"phase": "idle", "outcome": "measured", "profile_id": profile_id, **result},
    )


@pytest.mark.magik_profile
def test_motion_profile(application_session):
    app, agent, run, profile_id = application_session
    result = launcher_motion(app, agent, instrumented=True)
    append_event(
        run,
        {"phase": "motion", "outcome": "measured", "profile_id": profile_id, **result},
    )


@pytest.mark.magik_profile
def test_motion_rollover_profile(application_session):
    app, agent, run, profile_id = application_session
    result = launcher_motion(app, agent, instrumented=True, align_rollover=True)
    append_event(
        run,
        {
            "phase": "motion-rollover",
            "outcome": "measured",
            "profile_id": profile_id,
            **result,
        },
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
def test_journeys(journey_application_session, repetition, journey):
    _journeys(journey_application_session, repetition, journey)


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


@pytest.mark.parametrize("repetition", range(3))
def test_animation_roundtrip(journey_application_session, repetition):
    from magik.animation_benchmark import animation_roundtrip

    app, agent, run, _ = journey_application_session
    result = animation_roundtrip(app, agent, run, repetition)
    append_event(
        run,
        {
            "phase": "animation-roundtrip",
            "outcome": "measured",
            "repetition": repetition,
            **result,
        },
    )


def test_helper_scheduler(journey_application_session):
    """Instrumented policy diagnostic, separate from cadence acceptance runs."""
    import json
    import time
    from slint_testing import KeyPressedEvent, KeyReleasedEvent

    app, agent, run, _ = journey_application_session
    app.first_window.dispatch_event(KeyPressedEvent("\uf703"))
    observed = []
    try:
        time.sleep(0.4)
        for _ in range(3):
            report = agent.device_operation("input-probe", {"seconds": 2, "events": []})
            observed.append(report)
            snapshots = [report["runtime_before"], report["runtime"]]
            if any(
                t.get("policy") == 2 and t.get("rt_priority") == 1
                for snap in snapshots
                for p in snap["processes"]
                for t in p["threads"]
                if "card-tile" in t.get("name", "")
            ):
                break
    finally:
        app.first_window.dispatch_event(KeyReleasedEvent("\uf703"))
    time.sleep(1)
    settled = agent.device_operation("input-probe", {"seconds": 0, "events": []})
    (run / "helper-scheduler.json").write_text(
        json.dumps({"active": observed, "settled": settled}, indent=2)
    )
    snapshots = [s for r in observed for s in [r["runtime_before"], r["runtime"]]]
    assert any(
        t.get("policy") == 2 and t.get("rt_priority") == 1
        for s in snapshots
        for p in s["processes"]
        for t in p["threads"]
        if "card-tile" in t.get("name", "")
    ), "helper real-time policy never observed"
    for snap in snapshots + [settled["runtime"]]:
        for process in snap["processes"]:
            assert all(
                t.get("policy") == 0
                for t in process["threads"]
                if t["tid"] == str(process["pid"])
            )
    assert all(
        t.get("policy") == 0
        for p in settled["runtime"]["processes"]
        for t in p["threads"]
        if "card-tile" in t.get("name", "")
    )
