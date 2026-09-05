from unittest.mock import Mock
from magik2 import cli
from magik2.build import BuildResult
from magik2.compatibility import AgentStatus
from magik2.protocol import sha256_hex
from magik2.results import create_run


def test_published_new_artifact_does_not_mean_it_is_running(monkeypatch, tmp_path):
    artifact = tmp_path / "probe"
    artifact.write_bytes(b"new binary")
    sha = sha256_hex(artifact.read_bytes())
    monkeypatch.setattr(
        cli, "ensure_arm_probe", lambda *args: BuildResult(artifact, False, 0)
    )
    agent = Mock()
    status = AgentStatus(
        "other",
        frozenset(),
        {
            "running": True,
            "ready": True,
            "artifact_sha256": sha,
            "running_sha256": "old",
        },
    )
    assert not cli.ensure_probe(agent, status, create_run(tmp_path, "deploy", {}))
    agent.upload.assert_not_called()
    agent.start.assert_called_once_with(expected_sha256=sha, restart=True)


def test_deployment_has_no_testing_or_legacy_diagnostic_requirement():
    assert not any(
        "test" in name or "legacy" in name for name in cli.REQUIRED_AGENT_CAPABILITIES
    )
    assert cli.STATUS_CAPABILITIES == {"status"}
    assert "upload-v1" not in cli.STOP_CAPABILITIES
