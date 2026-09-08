from unittest.mock import Mock
import hashlib
import json
import subprocess
from types import SimpleNamespace
import zipfile

import pytest

from magik import updates, update_deploy
from magik.update_deploy import ensure_service_boot


def test_verification_progress_is_flushed(deploy_case, monkeypatch):
    _, current, _ = deploy_case
    writer = Mock()
    monkeypatch.setattr("builtins.print", writer)
    update_deploy.report_verification(current)
    assert writer.call_count == 2
    assert all(call.kwargs.get("flush") is True for call in writer.call_args_list)


def test_boot_registration_reconciles_lost_reply_without_replay():
    agent = Mock()

    def reply(ready):
        return SimpleNamespace(
            operation="service-boot-state", fields={"ready": ready}
        ), b""

    agent._request.side_effect = [reply(False), TimeoutError("reply lost"), reply(True)]
    update_deploy.ensure_service_boot(agent)
    assert [call.args[0] for call in agent._request.call_args_list] == [
        "service-boot-state",
        "service-boot-install",
        "service-boot-state",
    ]
    assert agent._request.call_args_list[1].kwargs["attempts"] == 1


def test_boot_registration_must_verify():
    agent = Mock()
    agent._request.return_value = (
        SimpleNamespace(operation="service-boot-state", fields={"ready": False}),
        b"",
    )
    with pytest.raises(RuntimeError, match="not verified"):
        update_deploy.ensure_service_boot(agent)


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
    monkeypatch.setattr(
        update_deploy, "ensure_service_boot", Mock(return_value={"ready": True})
    )
    publish = Mock()
    monkeypatch.setattr(update_deploy, "publish", publish)
    return pair, current, publish


def test_current_pair_is_noop_on_multiple_devices(deploy_case, tmp_path):
    pair, _, publish = deploy_case
    for identity in ("one", "two"):
        update_deploy.apply_updates(Mock(), identity, pair, tmp_path, False)
    publish.assert_not_called()
    assert len(list((updates.root() / "devices").glob("*.json"))) == 2


@pytest.mark.parametrize("recover", [False, True])
@pytest.mark.parametrize("database_change", [False, True])
@pytest.mark.parametrize("boot_ready", [False, True])
def test_boot_check_includes_recovery_and_current_platform(
    deploy_case, tmp_path, monkeypatch, recover, database_change, boot_ready
):
    pair, current, publish = deploy_case
    monkeypatch.setattr(update_deploy, "ensure_service_boot", ensure_service_boot)
    monkeypatch.setattr(update_deploy, "database_files", lambda *_: {"db": tmp_path})
    if database_change:
        current["databases"]["version"] = 1
    if recover:
        current["stages"] = [
            {
                "stage": "a" * 32,
                "pending": {"kind": "platform", "layout": "dev", "boot_id": "previous"},
            }
        ]
        previous = tmp_path / "previous"
        updates.atomic_json(previous / "platform/publication.json", {"stage": "a" * 32})
        updates.atomic_json(
            updates.root()
            / "devices"
            / f"{hashlib.sha256(b'one').hexdigest()}-pending.json",
            {"run": str(previous)},
        )
    agent = Mock()
    installed_boot = boot_ready

    def request(operation, fields, **kwargs):
        nonlocal installed_boot
        if operation == "publication-control":
            assert fields["action"] == "finish"
            current["stages"] = []
            return SimpleNamespace(operation="publication-complete", fields={}), b""
        if operation == "service-boot-install":
            assert kwargs["attempts"] == 1
            installed_boot = True
        else:
            assert operation == "service-boot-state"
        return SimpleNamespace(
            operation="service-boot-state", fields={"ready": installed_boot}
        ), b""

    agent._request.side_effect = request

    def published(*args, **kwargs):
        assert installed_boot
        assert kwargs["kind"] == "databases"
        current["databases"]["version"] = 2

    publish.side_effect = published
    update_deploy.apply_updates(agent, "one", pair, tmp_path, recover)
    operations = [call.args[0] for call in agent._request.call_args_list]
    assert operations.count("service-boot-install") == int(not boot_ready)
    assert publish.call_count == int(database_change)
    receipt = json.loads(
        (
            updates.root() / "devices" / f"{hashlib.sha256(b'one').hexdigest()}.json"
        ).read_text()
    )
    assert receipt["service_boot"]["ready"] is True


def test_boot_failure_blocks_completion_and_database_install(
    deploy_case, tmp_path, monkeypatch
):
    pair, current, publish = deploy_case
    current["databases"]["version"] = 1
    monkeypatch.setattr(update_deploy, "database_files", lambda *_: {"db": tmp_path})
    monkeypatch.setattr(
        update_deploy,
        "ensure_service_boot",
        Mock(side_effect=RuntimeError("boot unverified")),
    )
    with pytest.raises(RuntimeError, match="boot unverified"):
        update_deploy.apply_updates(Mock(), "one", pair, tmp_path, False)
    publish.assert_not_called()
    assert not (
        updates.root() / "devices" / f"{hashlib.sha256(b'one').hexdigest()}.json"
    ).exists()


def test_runtime_report_does_not_equate_active_with_verified_identity(
    deploy_case, capsys
):
    _, current, _ = deploy_case
    update_deploy.report_verification(current)
    output = capsys.readouterr().out
    assert "running Main identity: unavailable" in output
    assert "exact loaded identities unverified" in output
    current["platform"]["runtime_verification"] = {"main": {"matches_installed": True}}
    update_deploy.report_verification(current)
    assert "running Main identity: verified" in capsys.readouterr().out


def test_unrelated_corrupt_journal_does_not_block_current_device(deploy_case, tmp_path):
    pair, _, publish = deploy_case
    directory = updates.root() / "devices"
    directory.mkdir(parents=True)
    (directory / "other-pending.json").write_text("interrupted")
    update_deploy.apply_updates(Mock(), "one", pair, tmp_path, False)
    publish.assert_not_called()


def test_queued_deploy_starts_idle_application(deploy_case, tmp_path, monkeypatch):
    from argparse import Namespace
    from magik import cli
    from magik.build import BuildResult
    from magik.compatibility import AgentStatus
    from magik.protocol import sha256_hex
    from magik.results import create_run

    pair, current, publish = deploy_case
    current["running"]["launcher_ready_phase"] = "idle"
    artifact = tmp_path / "application"
    artifact.write_bytes(b"application")
    status = AgentStatus(
        "0.1.0",
        frozenset(),
        {
            "device_identity": "e2:b1:0a:86:84:3c",
            "running": False,
            "ready": False,
            "artifacts": {"magik": sha256_hex(b"application")},
        },
    )
    agent = Mock()
    agent.status.return_value = status
    monkeypatch.setattr(updates, "desired", lambda: pair)
    monkeypatch.setattr(cli, "connect_agent", lambda *_: (agent, status))
    monkeypatch.setattr(
        cli, "ensure_arm_application", lambda *_: BuildResult(artifact, False, 0)
    )
    monkeypatch.setattr(cli, "retain_diagnostics", lambda *_: None)
    assert (
        cli.deploy(
            Namespace(app="magik", attended=False), create_run(tmp_path, "deploy", {})
        )
        == 0
    )
    publish.assert_not_called()
    agent.upload.assert_not_called()
    agent.start.assert_called_once_with(
        expected_sha256=sha256_hex(b"application"), restart=False
    )
    assert (
        updates.root()
        / "devices"
        / f"{hashlib.sha256(b'e2:b1:0a:86:84:3c').hexdigest()}.json"
    ).exists()
    assert not (
        updates.root() / "devices" / f"{hashlib.sha256(b'0.1.0').hexdigest()}.json"
    ).exists()


@pytest.mark.parametrize("board_matches", [True, False])
@pytest.mark.parametrize("malformed_journal", [[], None, True, 17, "not-an-object"])
def test_legacy_journal_recovery_requires_board_and_stage(
    deploy_case, tmp_path, monkeypatch, board_matches, malformed_journal
):
    pair, current, publish = deploy_case
    previous_run = tmp_path / "previous"
    updates.atomic_json(
        previous_run / "run.json",
        {"source": {"device_identity": "one" if board_matches else "another"}},
    )
    updates.atomic_json(previous_run / "platform/publication.json", {"stage": "a" * 32})
    legacy = (
        updates.root()
        / "devices"
        / f"{hashlib.sha256(b'0.1.0').hexdigest()}-pending.json"
    )
    updates.atomic_json(legacy, {"desired": pair, "run": str(previous_run)})
    malformed_run = tmp_path / "malformed"
    updates.atomic_json(
        malformed_run / "run.json", {"source": {"device_identity": "one"}}
    )
    updates.atomic_json(malformed_run / "platform/publication.json", malformed_journal)
    malformed = legacy.parent / "malformed-pending.json"
    updates.atomic_json(malformed, {"desired": pair, "run": str(malformed_run)})
    # Visit the malformed same-board candidate first regardless of filesystem order.
    original_glob = update_deploy.Path.glob
    monkeypatch.setattr(
        update_deploy.Path,
        "glob",
        lambda path, pattern: iter([malformed, legacy])
        if path == legacy.parent and pattern == "*-pending.json"
        else original_glob(path, pattern),
    )
    current["stages"] = [
        {
            "stage": "a" * 32,
            "pending": {
                "kind": "platform",
                "layout": "dev",
                "boot_id": "previous-boot",
            },
        }
    ]
    agent = Mock()

    def finish(*args, **kwargs):
        assert args[0] == "publication-control"
        assert args[1]["action"] == "finish"
        assert kwargs["attempts"] == 1
        current["stages"] = []
        return SimpleNamespace(operation="publication-complete", fields={}), b""

    agent._request.side_effect = finish
    if board_matches:
        update_deploy.apply_updates(agent, "one", pair, tmp_path, True)
        agent._request.assert_called_once()
    else:
        with pytest.raises(RuntimeError, match="no reboot repeated"):
            update_deploy.apply_updates(agent, "one", pair, tmp_path, True)
        agent._request.assert_not_called()
    publish.assert_not_called()
    assert legacy.exists()


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
        kwargs["stdout"].write(
            json.dumps(
                {
                    "reason": "compiler-artifact",
                    "target": {"name": "mister-magik-manager", "kind": ["bin"]},
                    "executable": f"/workspace/mister/tools/manager/target/{TARGET}/release/mister-magik-manager",
                    "fresh": False,
                }
            )
            + "\n"
        )
        return SimpleNamespace(returncode=0)

    result = ensure_arm_package(
        package,
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
