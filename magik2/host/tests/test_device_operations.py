import argparse
import json
from types import SimpleNamespace
from unittest.mock import Mock
import pytest
from magik2 import device
from magik2.client import AgentError
from magik2.publication import publish


def parser():
    result = argparse.ArgumentParser()
    device.add_commands(result.add_subparsers(dest="device_command", required=True))
    return result


@pytest.mark.parametrize(
    "command",
    [
        "status",
        "diagnostics",
        "logs",
        "launcher status",
        "launcher restart",
        "launcher return-to-launcher",
        "mode status",
        "mode set dev --attended",
        "display status",
        "display set crt-240p60 --attended",
        "catalog inspect",
        "catalog cores",
        "catalog metadata-qualification",
        "catalog neogeo-family-audit",
        "catalog query --database system:arcade --sql SELECT",
        "catalog screenshots --system arcade",
        "catalog purge --confirm",
        "catalog publish --release-dir /tmp/release",
        "media check --system arcade",
    ],
)
def test_native_command_surface(command):
    parser().parse_args(command.split())


@pytest.mark.parametrize(
    "command",
    [
        "display set crt-240p60",
        "mode set public",
        "catalog purge",
        "catalog query --database registry",
        "catalog screenshots",
        "media download",
        "catalog rom-audit",
        "catalog query --database registry --sql SELECT",
        "catalog query --database library --sql SELECT",
        "catalog query --database system:../arcade --sql SELECT",
    ],
)
def test_mutation_and_query_inputs_are_explicit(command):
    with pytest.raises(SystemExit):
        parser().parse_args(command.split())


def test_publication_failure_retains_stage_and_does_not_retry(tmp_path):
    artifact = tmp_path / "artifact"
    artifact.write_bytes(b"data")
    agent = Mock()
    agent._request.side_effect = TimeoutError("ambiguous upload")
    with pytest.raises(TimeoutError):
        publish(
            agent,
            tmp_path,
            {"main": artifact},
            kind="local-main",
            layout="dev",
            attended=True,
        )
    assert agent._request.call_count == 1
    assert agent._request.call_args.kwargs["attempts"] == 1
    evidence = json.loads((tmp_path / "publication.json").read_text())
    assert evidence["stage"] and evidence["error"] == "ambiguous upload"


def test_catalog_raw_failure_is_saved_before_validation(monkeypatch, tmp_path):
    agent = Mock()
    agent._request.return_value = (
        SimpleNamespace(
            operation="catalog-result", fields={"exit_code": 2, "error": "failed"}
        ),
        b"partial report",
    )
    monkeypatch.setattr("magik2.cli.connect_agent", lambda *_: (agent, None))
    with pytest.raises(AgentError):
        device.run_catalog(SimpleNamespace(action="inspect", layout="dev"), tmp_path)
    assert (tmp_path / "catalog-output.txt").read_bytes() == b"partial report"


def test_platform_reboots_once_then_confirms_transaction(monkeypatch, tmp_path):
    agent = Mock()
    agent._request.side_effect = [
        (
            SimpleNamespace(
                operation="publication-complete", fields={"requires_reboot": True}
            ),
            b"",
        ),
        (
            SimpleNamespace(
                operation="publication-complete", fields={"activated": True}
            ),
            b"",
        ),
    ]
    reboot = Mock()
    monkeypatch.setattr(device, "reboot_device", reboot)
    result = publish(agent, tmp_path, {}, kind="platform", layout="dev", attended=True)
    reboot.assert_called_once()
    assert result["activation"]["activated"] is True
    assert agent._request.call_args.args[0] == "publication-control"


def test_failed_platform_reboot_keeps_recoverable_stage(monkeypatch, tmp_path):
    agent = Mock()
    agent._request.return_value = (
        SimpleNamespace(
            operation="publication-complete", fields={"requires_reboot": True}
        ),
        b"",
    )
    reboot = Mock(side_effect=AgentError("offline after reboot"))
    monkeypatch.setattr(device, "reboot_device", reboot)
    with pytest.raises(AgentError, match="offline"):
        publish(agent, tmp_path, {}, kind="platform", layout="dev", attended=True)
    assert agent._request.call_count == reboot.call_count == 1
    assert json.loads((tmp_path / "publication.json").read_text())["stage"]
