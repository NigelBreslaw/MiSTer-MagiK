import json

import pytest

from magik import discovery
from magik.device_profile import DeviceProfile, device_identity, migrate_token
from magik.keychain import KeychainError
from magik.token_store import TokenStore

IDENTITY = "02:12:34:56:78:90"


def test_native_only_recovery_never_reads_credentials_or_uses_ssh(monkeypatch):
    from unittest.mock import Mock

    DeviceProfile(IDENTITY, "192.168.1.99", "root").save()
    monkeypatch.setenv("MISTER_PASS", "must-not-use")
    keychain = Mock(side_effect=AssertionError("credential fallback"))
    ssh = Mock(side_effect=AssertionError("SSH fallback"))
    monkeypatch.setattr(discovery, "Keychain", keychain)
    monkeypatch.setattr(discovery, "SshBootstrap", ssh)
    monkeypatch.setattr(
        discovery, "native_identity", Mock(side_effect=ConnectionRefusedError())
    )
    with pytest.raises(discovery.DiscoveryError):
        discovery.resolve_device(native_only=True, expected_identity=IDENTITY)
    keychain.assert_not_called()
    ssh.assert_not_called()


@pytest.fixture(autouse=True)
def isolated(monkeypatch, tmp_path):
    monkeypatch.setenv("MISTER_MAGIK2_STATE", str(tmp_path))
    for key in ("MISTER_IP", "MISTER_USER", "MISTER_PASS"):
        monkeypatch.delenv(key, raising=False)
    monkeypatch.setattr(
        discovery, "local_candidates", lambda: ["192.168.1.2", "192.168.1.3"]
    )

    class Keychain:
        def load(self, *_):
            return None

        def save(self, *_):
            pass

    monkeypatch.setattr(discovery, "Keychain", Keychain)


def test_profile_is_nonsecret_and_migrates_verified_token(tmp_path):
    profile = DeviceProfile(IDENTITY, "192.168.1.2", "root")
    old = TokenStore(tmp_path, profile.address)
    old.save("fixture-token")
    profile.save(tmp_path)
    assert set(json.loads((tmp_path / "device.json").read_text())) == {
        "identity",
        "address",
        "username",
    }
    assert migrate_token(profile, tmp_path) == "fixture-token"
    assert TokenStore(tmp_path, IDENTITY).load() == "fixture-token"
    assert not old.path.exists()
    assert DeviceProfile.load(tmp_path) == profile


def test_stale_address_finds_same_identity(monkeypatch):
    DeviceProfile(IDENTITY, "192.168.1.99", "root").save()

    def identify(address, _timeout):
        if address == "192.168.1.2":
            return IDENTITY
        if address == "192.168.1.3":
            return "02:12:34:56:78:91"
        raise OSError("offline")

    monkeypatch.setattr(discovery, "native_identity", identify)
    assert discovery.resolve_device().address == "192.168.1.2"
    assert DeviceProfile.load().address == "192.168.1.2"


def test_multiple_devices_need_selection(monkeypatch):
    monkeypatch.setattr(
        discovery,
        "native_identity",
        lambda address, _: IDENTITY if address.endswith("2") else "02:12:34:56:78:91",
    )
    with pytest.raises(discovery.DiscoveryError, match="Multiple"):
        discovery.resolve_device()
    assert DeviceProfile.load() is None
    assert discovery.resolve_device("192.168.1.2", select=True).identity == IDENTITY


def test_denied_keychain_does_not_write_password(monkeypatch, tmp_path):
    monkeypatch.setenv("MISTER_PASS", "never-in-a-file")
    monkeypatch.setattr(discovery, "native_identity", lambda *_: IDENTITY)

    def denied(*_):
        raise KeychainError("access denied")

    monkeypatch.setattr(discovery.Keychain, "save", denied)
    with pytest.raises(KeychainError, match="access denied"):
        discovery.resolve_device("192.168.1.2", select=True)
    assert not list(tmp_path.iterdir())


def test_explicit_address_cannot_silently_change_device(monkeypatch):
    DeviceProfile(IDENTITY, "192.168.1.2", "root").save()
    monkeypatch.setattr(discovery, "native_identity", lambda *_: "02:12:34:56:78:91")
    with pytest.raises(discovery.DiscoveryError, match="different identity"):
        discovery.resolve_device("192.168.1.3")
    assert DeviceProfile.load().identity == IDENTITY


@pytest.mark.parametrize(
    "value", [None, "", "unknown", "00:00:00:00:00:00", "ff:ff:ff:ff:ff:ff"]
)
def test_invalid_identity(value):
    with pytest.raises(ValueError):
        device_identity(value)


def test_network_denial_is_specific(monkeypatch):
    import errno

    def denied(*_):
        raise PermissionError(errno.EACCES, "denied")

    monkeypatch.setattr(discovery, "native_identity", denied)
    with pytest.raises(discovery.DiscoveryError, match="permission denied"):
        discovery.resolve_device()


def test_discovery_returns_at_deadline_and_cancels_pending(monkeypatch):
    import time

    monkeypatch.setattr(discovery, "DISCOVERY_SECONDS", 0.04)

    def offline(*_):
        time.sleep(0.08)
        raise OSError("offline")

    monkeypatch.setattr(discovery, "native_identity", offline)
    started = time.monotonic()
    with pytest.raises(discovery.DiscoveryError, match="offline"):
        discovery.resolve_device()
    assert time.monotonic() - started < 0.075


def test_local_candidates_exclude_public_and_respect_small_netmask(monkeypatch):
    from types import SimpleNamespace

    def command(argv, **_):
        if argv[0].endswith("ifconfig"):
            output = (
                "inet 192.168.1.2 netmask 0xfffffffc\ninet 8.8.8.8 netmask 0xffffff00"
            )
        elif argv[0].endswith("arp"):
            output = "? (1.1.1.1) at 00:11:22:33:44:55"
        else:
            output = ""
        return SimpleNamespace(stdout=output)

    monkeypatch.undo()
    monkeypatch.setattr(discovery.subprocess, "run", command)
    assert discovery.local_candidates() == ["192.168.1.1", "192.168.1.2"]
