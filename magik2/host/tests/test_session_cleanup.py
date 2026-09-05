from contextlib import contextmanager
from unittest.mock import Mock
import pytest
from magik2 import scenario_runner
from magik2.results import create_run


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
