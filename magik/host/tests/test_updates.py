from unittest.mock import Mock
import hashlib
import json
import subprocess
from types import SimpleNamespace
import zipfile

import pytest

from magik import updates, update_deploy


def test_numbered_release_selection_includes_prereleases():
    selector = updates.selectors()
    releases = [
        {"tag_name": "platform-v0.10", "published_at": "2026", "prerelease": True},
        {"tag_name": "platform-v0.11", "published_at": "2026", "draft": True},
        {"tag_name": "platform-v0.9", "published_at": "2027"},
        {"tag_name": "game-databases-v12", "published_at": "2026", "prerelease": True},
    ]
    assert selector.durable_platforms(releases)[0]["tag_name"] == "platform-v0.10"
    assert selector.select_game_databases(releases)["tag_name"] == "game-databases-v12"


def test_update_failure_preserves_previous_pair(tmp_path, monkeypatch):
    monkeypatch.setenv("MISTER_MAGIK2_STATE", str(tmp_path))
    updates.atomic_json(updates.root() / "desired.json", {"old": True})
    monkeypatch.setattr(
        updates.subprocess,
        "run",
        Mock(side_effect=subprocess.CalledProcessError(1, "gh")),
    )
    with pytest.raises(subprocess.CalledProcessError):
        updates.update()
    assert json.loads((updates.root() / "desired.json").read_text()) == {"old": True}


def test_verify_rejects_corrupt_assets_before_contracts(tmp_path, monkeypatch):
    archive, manifest = updates.names("platform", 2)
    for name in (archive, manifest):
        (tmp_path / name).write_bytes(b"bad")
    (tmp_path / "SHA256SUMS").write_text(f"{'0' * 64}  {archive}\n")
    contracts = Mock()
    monkeypatch.setattr(updates, "contracts", contracts)
    with pytest.raises(ValueError, match="checksum"):
        updates.verify("platform", {"directory": str(tmp_path), "version": 2})
    contracts.assert_not_called()


@pytest.fixture
def deploy_case(tmp_path, monkeypatch):
    monkeypatch.setenv("MISTER_MAGIK2_STATE", str(tmp_path))
    manifest = tmp_path / "game-databases-manifest.json"
    manifest.write_text("database")
    pair = {
        kind: {"tag": kind, "version": 2, "directory": str(tmp_path)}
        for kind in ("platform", "databases")
    }
    current = {
        "platform": {
            "version": 2,
            "verified": True,
            "active": True,
            "bundle_id": "bundle",
        },
        "databases": {
            "version": 2,
            "verified": True,
            "hashes": {"manifest": hashlib.sha256(manifest.read_bytes()).hexdigest()},
        },
        "stages": [],
        "boot_id": "boot",
        "configured_main": "MiSTer_MagiKDev",
        "running": {"executable_path": "/media/fat/MiSTer_MagiKDev"},
    }
    current["platform"]["hashes"] = {
        name: "hash" for name in update_deploy.PLATFORM_MEMBERS
    }
    monkeypatch.setattr(
        update_deploy,
        "verify",
        lambda *_: {
            "bundle_id": "bundle",
            "files": [
                {"path": path, "sha256": "hash"}
                for path in update_deploy.PLATFORM_MEMBERS.values()
            ],
        },
    )
    monkeypatch.setattr(update_deploy, "state", lambda *_: current)
    monkeypatch.setattr(update_deploy, "ensure_arm_application", Mock())
    publish = Mock()
    monkeypatch.setattr(update_deploy, "publish", publish)
    return pair, current, publish


def test_current_pair_is_noop_on_multiple_devices(deploy_case, tmp_path):
    pair, _, publish = deploy_case
    for identity in ("one", "two"):
        update_deploy.apply_updates(Mock(), identity, pair, tmp_path, False)
    publish.assert_not_called()
    assert len(list((updates.root() / "devices").glob("*.json"))) == 2


def test_missing_attendance_prevents_all_publication(deploy_case, tmp_path):
    pair, current, publish = deploy_case
    current["platform"]["version"] = 1
    with pytest.raises(RuntimeError, match="deploy --attended"):
        update_deploy.apply_updates(Mock(), "one", pair, tmp_path, False)
    publish.assert_not_called()


def test_database_only_does_not_prepare_platform(deploy_case, tmp_path, monkeypatch):
    pair, current, publish = deploy_case
    current["databases"]["version"] = 1
    prepare = Mock(side_effect=AssertionError("unexpected platform build"))
    monkeypatch.setattr(update_deploy, "prepare", prepare)
    monkeypatch.setattr(update_deploy, "database_files", lambda *_: {"db": tmp_path})
    publish.side_effect = lambda *a, **kw: current["databases"].update(version=2)
    update_deploy.apply_updates(Mock(), "one", pair, tmp_path, False)
    assert publish.call_args.kwargs == {"kind": "databases", "layout": "dev"}


def test_ambiguous_stage_does_not_reboot(deploy_case, tmp_path):
    pair, current, publish = deploy_case
    current["stages"] = [
        {"stage": "a" * 32, "pending": {"layout": "dev", "boot_id": "boot"}}
    ]
    agent = Mock()
    with pytest.raises(RuntimeError, match="no reboot repeated"):
        update_deploy.apply_updates(agent, "one", pair, tmp_path, True)
    agent._request.assert_not_called()
    publish.assert_not_called()


def test_newer_invalid_release_never_downgrades():
    with pytest.raises(RuntimeError, match="refusing downgrade"):
        update_deploy.needed(
            {"version": 3, "verified": False}, {"version": 2}, "platform", {}
        )


def test_platform_success_database_failure_retries_only_database(
    deploy_case, tmp_path, monkeypatch
):
    pair, current, publish = deploy_case
    current["platform"]["version"] = 1
    current["databases"]["version"] = 1
    prepare = Mock(return_value={"platform": tmp_path})
    monkeypatch.setattr(update_deploy, "prepare", prepare)
    monkeypatch.setattr(update_deploy, "database_files", lambda *_: {"db": tmp_path})

    def fail_database(*args, **fields):
        if fields["kind"] == "platform":
            current["platform"]["version"] = 2
        else:
            raise RuntimeError("database transport lost")

    publish.side_effect = fail_database
    with pytest.raises(RuntimeError, match="transport lost"):
        update_deploy.apply_updates(Mock(), "one", pair, tmp_path, True)
    assert (
        updates.root()
        / "devices"
        / f"{hashlib.sha256(b'one').hexdigest()}-pending.json"
    ).exists()
    publish.reset_mock()
    publish.side_effect = lambda *a, **kw: current["databases"].update(version=2)
    update_deploy.apply_updates(Mock(), "one", pair, tmp_path, False)
    assert publish.call_count == 1
    assert publish.call_args.kwargs["kind"] == "databases"
    assert prepare.call_count == 1


def test_archive_path_escape_rejected(tmp_path):
    import zipfile

    archive = tmp_path / "bad.zip"
    with zipfile.ZipFile(archive, "w") as stream:
        stream.writestr("../outside", "bad")
    with pytest.raises(ValueError, match="unsafe"):
        update_deploy.extract(archive, tmp_path / "output")
    assert not (tmp_path / "outside").exists()


def test_paginated_download_then_cache_reuse(tmp_path, monkeypatch):
    monkeypatch.setenv("MISTER_MAGIK2_STATE", str(tmp_path))
    validator = SimpleNamespace(verify=lambda *_: {})
    monkeypatch.setattr(updates, "contracts", lambda: (validator, validator))
    calls = []

    def gh(args, **kwargs):
        calls.append(args)
        if args[1] == "api":
            return SimpleNamespace(
                stdout=json.dumps(
                    [
                        [
                            {
                                "tag_name": "platform-v0.4",
                                "published_at": "now",
                                "prerelease": True,
                            }
                        ],
                        [{"tag_name": "game-databases-v8", "published_at": "now"}],
                    ]
                )
            )
        kind, version = (
            ("platform", 4) if args[3] == "platform-v0.4" else ("databases", 8)
        )
        directory = Path(args[args.index("--dir") + 1])
        archive, manifest = updates.names(kind, version)
        payload = b"{}"
        checks = f"{hashlib.sha256(payload).hexdigest()}  {manifest}\n".encode()
        (directory / manifest).write_bytes(payload)
        (directory / "SHA256SUMS").write_bytes(checks)
        with zipfile.ZipFile(directory / archive, "w") as stream:
            stream.writestr(manifest, payload)
            stream.writestr("SHA256SUMS", checks)
        return SimpleNamespace(returncode=0)

    from pathlib import Path

    monkeypatch.setattr(updates.subprocess, "run", gh)
    assert updates.update() == 0
    pair = updates.desired()
    assert pair["platform"]["version"] == 4
    assert pair["databases"]["version"] == 8
    assert updates.update() == 0
    assert len([args for args in calls if args[1] == "release"]) == 2
    assert len([args for args in calls if args[1] == "api"]) == 2


def test_device_lock_serializes_same_identity(tmp_path, monkeypatch):
    import threading

    monkeypatch.setenv("MISTER_MAGIK2_STATE", str(tmp_path))
    started, acquired = threading.Event(), threading.Event()

    def second():
        started.set()
        with update_deploy.device_lock("same"):
            acquired.set()

    with update_deploy.device_lock("same"):
        worker = threading.Thread(target=second)
        worker.start()
        assert started.wait(2)
        assert not acquired.wait(0.05)
        with update_deploy.device_lock("another"):
            pass
    worker.join(2)
    assert acquired.is_set()


def test_manager_build_uses_repository_root_and_manager_binary(tmp_path):
    from magik.build import ensure_arm_package, TARGET

    package = tmp_path / "mister/tools/manager"
    package.mkdir(parents=True)
    artifact = package / "target" / TARGET / "release/mister-magik-manager"
    commands, roots = [], []

    def runner(command, **kwargs):
        commands.append(command)
        artifact.parent.mkdir(parents=True, exist_ok=True)
        artifact.write_bytes(b"manager")
        return SimpleNamespace(returncode=0)

    result = ensure_arm_package(
        package,
        package / "target/cache.json",
        runner=runner,
        prepare=lambda repo, _: roots.append(repo) or "builder",
    )
    assert result.artifact == artifact
    assert roots == [tmp_path]
    assert "/workspace/mister/tools/manager" in commands[0]
    assert commands[0][commands[0].index("--bin") + 1] == "mister-magik-manager"


def test_unhealthy_installed_platform_is_not_rebooted(deploy_case, tmp_path):
    pair, current, publish = deploy_case
    current["platform"]["active"] = False
    with pytest.raises(RuntimeError, match="no reboot attempted"):
        update_deploy.apply_updates(Mock(), "one", pair, tmp_path, True)
    publish.assert_not_called()


def test_local_main_change_requires_release_install_but_gui_change_does_not(
    deploy_case, tmp_path
):
    pair, current, publish = deploy_case
    current["platform"]["hashes"]["gui"] = "local edit"
    update_deploy.apply_updates(Mock(), "one", pair, tmp_path, False)
    publish.assert_not_called()
    current["platform"]["hashes"]["main"] = "different Main with same bundle id"
    with pytest.raises(RuntimeError, match="deploy --attended"):
        update_deploy.apply_updates(Mock(), "one", pair, tmp_path, False)
