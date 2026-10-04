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
        next(scenario_runner._application_session(request, tmp_path))
    assert ("metrics-body-16m-v1" in connect.call_args.args[1]) is required
