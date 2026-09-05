"""Small pytest fixtures for native device sessions; scenarios live with consumers."""
from __future__ import annotations
import json
from contextlib import contextmanager
import math
import os
import statistics
import uuid
from pathlib import Path
import pytest
from .results import append_event, create_run, source_context
from .testing import fresh_session


def pytest_addoption(parser):
    group = parser.getgroup("magik2")
    group.addoption("--magik2-device", action="store_true", help="Run scenarios on the configured MiSTer")
    group.addoption("--magik2-profile", action="store_true", help="Add the separate instrumented motion repetition")
    group.addoption("--magik2-run", help="Existing result directory for this invocation")


def pytest_configure(config):
    config.addinivalue_line("markers", "magik2_profile: separate instrumented repetition")


@pytest.fixture(scope="session")
def magik2_run(request):
    if not request.config.getoption("--magik2-device"):
        pytest.skip("enable --magik2-device for hardware scenarios")
    value = request.config.getoption("--magik2-run")
    run = Path(value) if value else create_run(Path("build/magik2-results"), "check", source_context(os.environ.get("MISTER_IP", "")))
    request.config._magik2_run = run
    return run


@pytest.fixture
def probe_session(request, magik2_run):
    from .cli import connect_agent, ensure_probe, CHECK_AGENT_CAPABILITIES, PROFILE_AGENT_CAPABILITIES
    profiled = request.node.get_closest_marker("magik2_profile") is not None
    if profiled and not request.config.getoption("--magik2-profile"):
        pytest.skip("profiling not requested")
    profile_id = f"{magik2_run.name}-{uuid.uuid4().hex[:8]}" if profiled else None
    agent, status = connect_agent(magik2_run, PROFILE_AGENT_CAPABILITIES if profiled else CHECK_AGENT_CAPABILITIES)
    ensure_probe(agent, status, magik2_run)
    with managed_session(agent, magik2_run, profile_id, request.node.nodeid) as application:
        yield application, agent, magik2_run, profile_id


@contextmanager
def managed_session(agent, run, profile_id, scenario):
    profiled = profile_id is not None
    try:
        with fresh_session(agent, profile_id=profile_id) as application:
            yield application
    finally:
        errors = []
        if profiled:
            try:
                complete = json.loads(agent.read_profile_artifact(profile_id, "profile.json"))
                if complete.get("run_id") != profile_id or complete.get("sha256") != agent.expected_sha256 or not complete.get("complete") or complete.get("samples", 0) <= 0:
                    raise AssertionError("profile has no matching completed sample evidence")
                for name in ("profile.json", "profile.folded", "flamegraph.svg"):
                    data = agent.read_profile_artifact(profile_id, name)
                    if not data:
                        raise AssertionError(f"empty profile artifact: {name}")
                    (run / name).write_bytes(data)
                append_event(run, {"phase":"profile", "outcome":"retained", "instrumented":True, **complete})
            except Exception as error:
                errors.append(f"profile: {error}")
        try:
            agent.start(expected_sha256=agent.expected_sha256)
        except Exception as error:
            errors.append(f"persistent restore: {error}")
        append_event(run, {"phase":"cleanup", "scenario":scenario, "outcome":"failed" if errors else "passed", "errors":errors})
        if errors:
            pytest.fail("; ".join(errors))


@pytest.hookimpl(hookwrapper=True)
def pytest_runtest_makereport(item, call):
    outcome = yield
    report = outcome.get_result()
    run = getattr(item.config, "_magik2_run", None)
    if run is not None:
        append_event(run, {"phase":"assertion", "scenario":report.nodeid, "stage":report.when, "outcome":report.outcome})
        if report.failed:
            with (run / "pytest.log").open("a") as output:
                output.write(report.longreprtext[-16000:] + "\n")


def pytest_sessionfinish(session, exitstatus):
    run = getattr(session.config, "_magik2_run", None)
    if run is not None:
        summarize(run)


def summarize(run: Path) -> None:
    events = [json.loads(line) for line in (run / "events.jsonl").read_text().splitlines()]
    samples = [e for e in events if e.get("phase") == "motion" and e.get("outcome") == "measured" and e.get("instrumented") is False]
    if not samples:
        return
    values = sorted(e["render_us_total"] / e["presentations"] for e in samples)
    append_event(run, {"phase":"benchmark", "instrumented":False, "repetitions":len(samples), "render_us_per_frame":{"min":values[0],"median":statistics.median(values),"p95":values[math.ceil(.95*len(values))-1]}, "samples":samples})
