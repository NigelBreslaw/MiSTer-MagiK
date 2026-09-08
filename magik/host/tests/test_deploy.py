from unittest.mock import Mock
from magik import cli, updates
from magik.build import BuildResult
from magik.compatibility import AgentStatus
from magik.protocol import sha256_hex
from magik.results import create_run


def test_published_new_artifact_does_not_mean_it_is_running(monkeypatch, tmp_path):
    artifact = tmp_path / "probe"
    artifact.write_bytes(b"new binary")
    sha = sha256_hex(artifact.read_bytes())
    monkeypatch.setattr(
        cli, "ensure_arm_application", lambda *args: BuildResult(artifact, False, 0)
    )
    agent = Mock()
    status = AgentStatus(
        "other",
        frozenset(),
        {
            "running": True,
            "ready": True,
            "artifact_sha256": sha,
            "artifacts": {"mini-magik": sha},
            "running_sha256": "old",
        },
    )
    assert not cli.ensure_application(agent, status, create_run(tmp_path, "deploy", {}))
    agent.upload.assert_not_called()
    agent.start.assert_called_once_with(expected_sha256=sha, restart=True)


def test_deployment_has_no_testing_or_legacy_diagnostic_requirement():
    assert not any(
        "test" in name or "legacy" in name for name in cli.REQUIRED_AGENT_CAPABILITIES
    )
    assert cli.STATUS_CAPABILITIES == {"status"}
    assert "upload-v1" not in cli.STOP_CAPABILITIES


def test_real_app_uses_the_same_delivery_with_its_own_artifact(monkeypatch, tmp_path):
    artifact = tmp_path / "real-app"
    artifact.write_bytes(b"real app")
    builds = []

    def build(package):
        builds.append(package)
        return BuildResult(artifact, False, 0)

    monkeypatch.setattr(cli, "ensure_arm_application", build)
    agent = Mock()
    status = AgentStatus(
        "future-branch", frozenset(), {"running": False, "artifacts": {}}
    )
    assert not cli.ensure_application(
        agent, status, create_run(tmp_path, "deploy", {}), "magik"
    )
    assert builds[0].parts[-2:] == ("apps", "mister")
    assert agent.artifact == "magik"
    agent.upload.assert_called_once_with("magik", b"real app")
    agent.start.assert_called_once_with(
        expected_sha256=sha256_hex(b"real app"), restart=False
    )


def test_real_deploy_requires_input_proxy_but_mini_does_not(monkeypatch, tmp_path):
    monkeypatch.setenv("MISTER_MAGIK2_STATE", str(tmp_path / "state"))
    from argparse import Namespace
    from magik.apps import application

    monkeypatch.setattr(updates, "desired", lambda: None)
    required = []
    agent = Mock()
    monkeypatch.setattr(
        cli,
        "connect_agent",
        lambda run, capabilities: (required.append(capabilities) or agent, Mock()),
    )
    monkeypatch.setattr(cli, "ensure_application", lambda *args: True)
    monkeypatch.setattr(cli, "retain_diagnostics", lambda *args: None)
    assert cli.deploy(Namespace(app="magik"), create_run(tmp_path, "deploy", {})) == 0
    assert "main-managed-magik" in required[0]
    assert "main-managed-magik" not in application("mini-magik").agent_capabilities


def test_unchanged_ready_artifact_skips_upload_and_restart(monkeypatch, tmp_path):
    artifact = tmp_path / "application"
    artifact.write_bytes(b"same binary")
    digest = sha256_hex(artifact.read_bytes())
    build = Mock(return_value=BuildResult(artifact, False, 0))
    monkeypatch.setattr(cli, "ensure_arm_application", build)
    agent = Mock()
    status = AgentStatus(
        "device",
        frozenset(),
        {
            "running": True,
            "ready": True,
            "artifact": "magik",
            "running_sha256": digest,
            "artifacts": {"magik": digest},
        },
    )
    assert cli.ensure_application(
        agent, status, create_run(tmp_path, "deploy", {}), "magik"
    )
    build.assert_called_once()
    agent.upload.assert_not_called()
    agent.start.assert_not_called()
