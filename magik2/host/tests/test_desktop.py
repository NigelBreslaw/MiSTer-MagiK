import json
from types import SimpleNamespace

from magik2 import cli, desktop
from magik2.device_profile import DeviceProfile


def test_prepare_reuses_connection_and_never_prints_secrets(
    monkeypatch, capsys, tmp_path
):
    profile = DeviceProfile("aa:bb:cc:dd:ee:ff", "192.168.1.2", "root")
    monkeypatch.setattr(DeviceProfile, "load", lambda: profile)

    def connect(run, required):
        assert run == tmp_path
        assert required == desktop.CAPABILITIES
        print("build progress")
        return None, SimpleNamespace(
            fields={"device_identity": profile.identity}, capabilities=required
        )

    monkeypatch.setattr(cli, "connect_agent", connect)
    assert desktop.prepare(tmp_path) == 0
    output = capsys.readouterr()
    assert "build progress" in output.err
    assert json.loads(output.out)["identity"] == profile.identity
    assert "token" not in output.out


def test_prepare_reports_keychain_failure_as_json(monkeypatch, capsys, tmp_path):
    def connect(*_args):
        raise RuntimeError("macOS Keychain access denied")

    monkeypatch.setattr(cli, "connect_agent", connect)
    assert desktop.prepare(tmp_path) == 2
    assert json.loads(capsys.readouterr().out) == {
        "outcome": "error",
        "detail": "macOS Keychain access denied",
    }
