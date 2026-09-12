from types import SimpleNamespace
from unittest.mock import Mock

import pytest

from magik import platform as platform_cli


def test_development_install_is_explicit_dev_only(monkeypatch, tmp_path):
    release = {"tag": "platform-development-618-v0.12-0123456789abcdef"}
    files = {"manifest": tmp_path / "platform-v3.manifest"}
    agent = Mock()
    download = Mock(return_value=release)
    prepare = Mock(return_value=files)
    connect = Mock(return_value=(agent, {}))
    state = Mock(
        return_value={
            "configured_main": "MiSTer_MagiKDev",
            "running": {"executable_path": "/media/fat/MiSTer_MagiKDev"},
        }
    )
    boot = Mock()
    publish = Mock(return_value={"stage": "verified"})
    monkeypatch.setattr("magik.updates.development_platform_618", download)
    monkeypatch.setattr("magik.update_deploy.prepare", prepare)
    monkeypatch.setattr("magik.cli.connect_agent", connect)
    monkeypatch.setattr("magik.update_deploy.state", state)
    monkeypatch.setattr("magik.update_deploy.ensure_service_boot", boot)
    monkeypatch.setattr(platform_cli, "publish", publish)

    arguments = SimpleNamespace(
        tag=release["tag"], attended=True, activate_fpga=True
    )
    assert platform_cli.install_development_618(arguments, tmp_path) == 0
    download.assert_called_once_with(release["tag"])
    prepare.assert_called_once()
    connect.assert_called_once_with(
        tmp_path,
        {"publication-v1", "platform-publication-v1", "service-boot-v1"},
    )
    boot.assert_called_once_with(agent)
    publish.assert_called_once_with(
        agent,
        tmp_path,
        files,
        kind="platform",
        layout="dev",
        attended=True,
        activate_fpga=True,
    )


def test_development_install_rejects_non_dev_device(monkeypatch, tmp_path):
    agent = Mock()
    monkeypatch.setattr(
        "magik.updates.development_platform_618",
        Mock(return_value={"tag": "platform-development-618-v0.12-0123456789abcdef"}),
    )
    monkeypatch.setattr("magik.update_deploy.prepare", Mock(return_value={}))
    monkeypatch.setattr("magik.cli.connect_agent", Mock(return_value=(agent, {})))
    monkeypatch.setattr(
        "magik.update_deploy.state",
        Mock(
            return_value={
                "configured_main": "MiSTer",
                "running": {"executable_path": "/media/fat/MiSTer"},
            }
        ),
    )
    publish = Mock()
    monkeypatch.setattr(platform_cli, "publish", publish)
    with pytest.raises(RuntimeError, match="running and selected Dev mode"):
        platform_cli.install_development_618(
            SimpleNamespace(
                tag="platform-development-618-v0.12-0123456789abcdef"
            ),
            tmp_path,
        )
    publish.assert_not_called()
