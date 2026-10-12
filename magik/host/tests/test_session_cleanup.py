from contextlib import contextmanager
from unittest.mock import Mock
import pytest
from magik import scenario_runner
from magik.results import create_run


@contextmanager
def failed_attach(*args, **kwargs):
    raise RuntimeError("attach failed after spawn")
    yield


def test_partial_attachment_restores_the_same_artifact(monkeypatch, tmp_path):
    agent = Mock(expected_sha256="A")
    monkeypatch.setattr(scenario_runner, "fresh_session", failed_attach)
    with pytest.raises(RuntimeError, match="attach failed"):
        with scenario_runner.managed_session(
            agent, create_run(tmp_path, "check", {}), None, "smoke"
        ):
            pass
    agent.start.assert_called_once_with(expected_sha256="A")


def test_incomplete_or_stale_profile_fails_cleanup(monkeypatch, tmp_path):
    @contextmanager
    def session(*args, **kwargs):
        yield object()

    agent = Mock(expected_sha256="A")
    agent.read_profile_artifact.return_value = (
        b'{"complete":true,"run_id":"old","sha256":"A","samples":10}'
    )
    monkeypatch.setattr(scenario_runner, "fresh_session", session)
    with pytest.raises(pytest.fail.Exception, match="matching completed"):
        with scenario_runner.managed_session(
            agent, create_run(tmp_path, "check", {}), "new", "motion-profile"
        ):
            pass
    agent.start.assert_called_once_with(expected_sha256="A")


def test_route_profiles_are_retained_independently(monkeypatch, tmp_path):
    import json

    @contextmanager
    def session(*args, **kwargs):
        yield object()

    agent = Mock(expected_sha256="A")

    def artifact(profile_id, name):
        if name == "profile.json":
            return json.dumps(
                {"complete": True, "run_id": profile_id, "sha256": "A", "samples": 10}
            ).encode()
        return (profile_id + ";draw 10").encode()

    agent.read_profile_artifact.side_effect = artifact
    monkeypatch.setattr(scenario_runner, "fresh_session", session)
    run = create_run(tmp_path, "check", {})
    for profile_id in ("root", "settings"):
        with scenario_runner.managed_session(agent, run, profile_id, profile_id):
            pass
    for profile_id in ("root", "settings"):
        assert (
            run / "profiles" / profile_id / "profile.folded"
        ).read_text() == profile_id + ";draw 10"
    assert (run / "profile.folded").read_text() == "settings;draw 10"


@pytest.mark.parametrize(
    "mode,profiled,required",
    [
        ("off", False, False),
        ("neighbors", False, True),
        ("phases", False, True),
        ("off", True, False),
    ],
)
def test_extended_metrics_capability_is_negotiated_only_for_detailed_capture(
    monkeypatch, tmp_path, mode, profiled, required
):
    from types import SimpleNamespace
    from magik import cli

    options = {
        "--magik-profile": profiled,
        "--magik-frame-evidence": mode,
        "--magik-app": "magik",
    }
    request = SimpleNamespace(config=SimpleNamespace(getoption=options.__getitem__))
    connect = Mock(side_effect=RuntimeError("stop before device connection"))
    monkeypatch.setattr(cli, "connect_agent", connect)
    with pytest.raises(RuntimeError, match="stop before device connection"):
        scenario_runner._prepare_application(request, tmp_path, profiled)
    assert ("metrics-body-32m-v1" in connect.call_args.args[1]) is required


@pytest.mark.parametrize("installed", [False, True])
@pytest.mark.parametrize("matching", [False, True])
def test_real_session_validates_renderer_before_scenario(
    monkeypatch, tmp_path, installed, matching
):
    from contextlib import contextmanager
    from types import SimpleNamespace
    from magik import cli, measurement_contract

    monkeypatch.setenv("MAGIK_CARD_SAMPLER_AB", "axis")
    monkeypatch.setenv("MAGIK_CARD_QUANTISER", "fast")
    sha = "a" * 64
    options = {
        "--magik-profile": False,
        "--magik-frame-evidence": "off",
        "--magik-app": "magik",
        "--magik-installed-sha256": sha if installed else None,
        "--magik-reuse-installed": False,
    }
    request = SimpleNamespace(config=SimpleNamespace(getoption=options.__getitem__))
    context = {
        "card_sampler": "independent-vertical-prefilter" if matching else "current",
        "card_quantiser": "centred-bayer-shifts",
    }
    agent = SimpleNamespace(
        metrics=lambda: {"context": context}, start=Mock(), expected_sha256=sha
    )
    status = SimpleNamespace(
        fields={
            "running": True,
            "ready": True,
            "artifact": "magik",
            "running_sha256": sha,
        }
    )
    agent.status = lambda: status
    monkeypatch.setattr(cli, "connect_agent", lambda *_: (agent, status))
    monkeypatch.setattr(cli, "ensure_application", lambda *_: None)
    monkeypatch.setattr(measurement_contract, "verify_display", lambda *_: None)

    @contextmanager
    def session(*args):
        yield "app"

    monkeypatch.setattr(scenario_runner, "managed_session", session)
    agent = scenario_runner._prepare_application(request, tmp_path, False)
    generator = scenario_runner._application_session(request, tmp_path, agent, False)
    if matching:
        assert next(generator)[0] == "app"
        generator.close()
    else:
        with pytest.raises(AssertionError, match="does not match"):
            next(generator)


@pytest.mark.parametrize("reuse", [False, True])
@pytest.mark.parametrize(
    "changed",
    [
        {"running": False},
        {"ready": False},
        {"running_sha256": "b" * 64},
        {"artifact": "mini-magik"},
    ],
)
def test_installed_binary_is_rechecked_before_each_lease(
    monkeypatch, tmp_path, reuse, changed
):
    from types import SimpleNamespace
    from magik import measurement_contract

    sha = "a" * 64
    fields = {
        "running": True,
        "ready": True,
        "artifact": "magik",
        "running_sha256": sha,
    }
    options = {
        "--magik-app": "magik",
        "--magik-installed-sha256": None if reuse else sha,
        "--magik-reuse-installed": reuse,
    }
    request = SimpleNamespace(config=SimpleNamespace(getoption=options.__getitem__))
    agent = Mock(artifact="magik", expected_sha256=sha)
    agent.status.return_value = SimpleNamespace(fields=fields)
    agent.metrics.return_value = {
        "context": {"card_sampler": "current", "card_quantiser": "existing-bayer"}
    }
    agent.diagnostics.return_value = {}
    leases = []

    @contextmanager
    def session(*args):
        leases.append(object())
        yield leases[-1]

    monkeypatch.setattr(scenario_runner, "managed_session", session)
    monkeypatch.setattr(measurement_contract, "verify_display", lambda *_: None)
    run = create_run(tmp_path, "check", {})
    first = scenario_runner._application_session(request, run, agent, False)
    next(first)
    first.close()
    fields.update(changed)
    with pytest.raises(AssertionError, match="SHA-256 or is not ready"):
        next(scenario_runner._application_session(request, run, agent, False))
    assert agent.status.call_count == 2
    agent.start.assert_not_called()
    assert len(leases) == 1


def test_prepared_binary_is_restarted_before_the_next_lease(monkeypatch, tmp_path):
    from types import SimpleNamespace
    from magik import measurement_contract

    sha = "a" * 64
    options = {
        "--magik-app": "mini-magik",
        "--magik-installed-sha256": None,
        "--magik-reuse-installed": False,
    }
    request = SimpleNamespace(config=SimpleNamespace(getoption=options.__getitem__))
    agent = Mock(artifact="mini-magik", expected_sha256=sha)
    ready = False
    leases = []

    def start(*, expected_sha256):
        nonlocal ready
        assert expected_sha256 == sha
        ready = True

    @contextmanager
    def session(*args):
        nonlocal ready
        assert ready
        leases.append(object())
        try:
            yield leases[-1]
        finally:
            ready = False  # A lost persistent launcher between journeys.

    agent.start.side_effect = start
    monkeypatch.setattr(scenario_runner, "managed_session", session)
    monkeypatch.setattr(measurement_contract, "verify_display", lambda *_: None)
    run = create_run(tmp_path, "check", {})
    for _ in range(2):
        lease = scenario_runner._application_session(request, run, agent, False)
        next(lease)
        with pytest.raises(StopIteration):
            next(lease)
    assert len(leases) == agent.start.call_count == 2
