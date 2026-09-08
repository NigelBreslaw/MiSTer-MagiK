from __future__ import annotations

import copy
import json
import os
import subprocess
import sys
from pathlib import Path
from types import SimpleNamespace

import pytest

from magik import storage
from magik.storage import (
    IDLE_SECONDS,
    LABEL,
    Storage,
    atomic_json,
    container_name,
    guest_idle,
    recipe_key,
)


class ContainerHost:
    """Model the external lifecycle, including workloads surviving host disconnects."""

    def __init__(self):
        self.entries = []
        self.images = {}
        self.processes = {}
        self.calls = []
        self.fail = None
        self.ambiguous_delete = False

    def __call__(self, command, **kwargs):
        self.calls.append(command)
        args = command[1:]
        if self.fail and args[: len(self.fail)] == self.fail:
            raise subprocess.TimeoutExpired(command, 30)
        output = ""
        if args[:2] == ["list", "--all"]:
            output = json.dumps(self.entries)
        elif args[:2] == ["image", "list"]:
            output = json.dumps(
                [
                    {
                        "configuration": {
                            "name": "docker.io/library/" + ref,
                            "descriptor": {"digest": digest},
                        }
                    }
                    for ref, digest in self.images.items()
                ]
            )
        elif args[0] == "exec":
            output = self.processes.get(
                args[1], "1 0 Ss .cz-init\n2 1 S sleep\n9 0 R ps\n"
            )
        elif args[0] == "stop":
            next(e for e in self.entries if e["id"] == args[1])["status"]["state"] = (
                "stopped"
            )
        elif args[0] == "start":
            next(e for e in self.entries if e["id"] == args[1])["status"]["state"] = (
                "running"
            )
        elif args[0] == "delete":
            self.entries[:] = [e for e in self.entries if e["id"] != args[1]]
            if self.ambiguous_delete:
                raise subprocess.TimeoutExpired(command, 30)
        elif args[:2] == ["image", "delete"]:
            del self.images[args[2]]
        elif args[0] == "run":
            labels = dict(
                args[i + 1].split("=", 1)
                for i, arg in enumerate(args)
                if arg == "--label"
            )
            repository = Path(labels[LABEL + "checkout"])
            recipe = labels[LABEL + "recipe"]
            item = entry(repository, recipe)
            item["configuration"]["labels"] = labels
            self.entries.append(item)
        elif args[0] == "build":
            ref = args[args.index("--tag") + 1]
            self.images[ref] = "sha256:" + ref.split(":")[1]
        elif args[:2] == ["system", "df"]:
            output = "{}"
        else:
            raise AssertionError(command)
        return SimpleNamespace(stdout=output, stderr="", returncode=0)


def entry(repository, recipe, *, managed=True):
    image = f"magik-build:{recipe}"
    return {
        "id": container_name(repository, recipe),
        "configuration": {
            "creationDate": "2026-09-07T00:00:00Z",
            "labels": {
                LABEL + "version": "1",
                LABEL + "checkout": str(repository.resolve()),
                LABEL + "recipe": recipe,
            }
            if managed
            else {},
            "image": {"reference": image, "descriptor": {"digest": "sha256:" + recipe}},
            "mounts": [
                {"source": str(repository.resolve()), "destination": "/workspace"}
            ],
        },
        "status": {"state": "running"},
    }


@pytest.fixture
def setup(tmp_path, monkeypatch):
    repository = tmp_path / "repo"
    (repository / "magik/build").mkdir(parents=True)
    (repository / "magik/build/Containerfile").write_text("FROM ubuntu:20.04\n")
    data = tmp_path / "apple"
    data.mkdir()
    now = [100_000.0]
    host = ContainerHost()
    host.images[f"magik-build:{recipe_key(repository)}"] = "sha256:" + recipe_key(
        repository
    )
    manager = Storage(
        root=tmp_path / "state", runner=host, clock=lambda: now[0], data_root=data
    )
    monkeypatch.setenv("MISTER_MAGIK2_BUILD_CACHE", str(tmp_path / "cargo"))
    return manager, host, repository, now


def add(setup, name="other", *, age=0, managed=True, recipe=None):
    manager, host, repository, now = setup
    owner = repository.parent / name
    owner.mkdir(exist_ok=True)
    item = entry(owner, recipe or recipe_key(repository), managed=managed)
    if managed:
        item["configuration"]["labels"][LABEL + "state"] = str(manager.root.resolve())
    host.entries.append(item)
    (manager.data_root / "containers" / item["id"]).mkdir(parents=True)
    host.images[item["configuration"]["image"]["reference"]] = item["configuration"][
        "image"
    ]["descriptor"]["digest"]
    if managed:
        manager.touch(item)
        path = manager.record_path(item["id"])
        data = json.loads(path.read_text())
        data["last_used"] -= age
        atomic_json(path, data)
    return item


def test_warm_reuse_preserves_mounts_and_uses_init(setup):
    manager, host, repo, now = setup
    with manager.build_session(repo):
        name = manager.prepare(repo)
    now[0] += 10
    with manager.build_session(repo):
        assert manager.prepare(repo) == name
    runs = [c for c in host.calls if c[1] == "run"]
    assert len(runs) == 1 and "--init" in runs[0]
    assert f"{repo.resolve()}:/workspace" in runs[0]
    assert any(c.endswith(":/root/.cargo/registry") for c in runs[0])
    assert manager.last_used(host.entries[0]) == now[0]


def test_stopped_container_restarts_and_busy_guest_blocks_reuse(setup):
    manager, host, repo, _ = setup
    with manager.build_session(repo):
        name = manager.prepare(repo)
    host.entries[0]["status"]["state"] = "stopped"
    with manager.build_session(repo):
        manager.prepare(repo)
    assert ["container", "start", name] in host.calls
    host.processes[name] = "1 0 Ss .cz-init\n2 1 S sleep\n42 0 R rustc\n43 0 R ps\n"
    with pytest.raises(RuntimeError, match="active guest"):
        with manager.build_session(repo):
            manager.prepare(repo)


def test_expiry_missing_checkout_and_lru(setup):
    manager, host, repo, now = setup
    expired = add(setup, "expired", age=IDLE_SECONDS)
    missing = add(setup, "missing")
    Path(missing["configuration"]["labels"][LABEL + "checkout"]).rmdir()
    warm = [add(setup, f"warm{i}", age=100 - i) for i in range(5)]
    preview = manager.inspect(repo, measure=False)
    assert {r["id"] for r in preview["containers"] if r["eligible"]} == {
        expired["id"],
        missing["id"],
        warm[0]["id"],
    }
    assert not any(c[1] in {"stop", "delete"} for c in host.calls)
    result = manager.inspect(repo, apply=True, measure=False)
    assert not result["errors"]
    assert {e["id"] for e in host.entries} == {e["id"] for e in warm[1:]}


def test_all_idle_preserves_active_unknown_and_unmanaged(setup):
    manager, host, repo, _ = setup
    add(setup, "idle")
    busy = add(setup, "busy")
    host.processes[busy["id"]] = "1 0 Ss sleep\n55 0 S cargo\n56 0 R ps\n"
    unmanaged = add(setup, "legacy", managed=False)
    bad = add(setup, "corrupt")
    manager.record_path(bad["id"]).write_text("broken")
    result = manager.inspect(repo, apply=True, all_idle=True, measure=False)
    assert {e["id"] for e in host.entries} == {busy["id"], unmanaged["id"], bad["id"]}
    assert len(result["errors"]) == 1
    assert (
        next(r for r in result["containers"] if r["id"] == unmanaged["id"])["ownership"]
        == "migration-candidate"
    )


@pytest.mark.parametrize(
    "change", ["ownership", "creation", "future", "nan", "missing", "state"]
)
def test_uncertain_identity_or_metadata_never_deleted(setup, change):
    manager, host, repo, now = setup
    item = add(setup, age=IDLE_SECONDS)
    record = manager.record_path(item["id"])
    if change == "ownership":
        item["configuration"]["mounts"][0]["source"] = "/different"
    elif change == "creation":
        item["configuration"]["creationDate"] = "replacement"
    elif change == "missing":
        record.unlink()
    elif change == "state":
        item["status"]["state"] = "starting"
    else:
        data = json.loads(record.read_text())
        data["last_used"] = now[0] + 10 if change == "future" else float("nan")
        atomic_json(record, data)
    result = manager.inspect(repo, apply=True, all_idle=True, measure=False)
    assert result["errors"] and len(host.entries) == 1


def test_inspection_failure_and_failed_deletion_protect_resources(setup):
    manager, host, repo, _ = setup
    item = add(setup, age=IDLE_SECONDS)
    for command in (["exec"], ["stop"], ["delete"]):
        host.fail = command
        result = manager.inspect(repo, apply=True, measure=False)
        assert result["errors"] and host.entries
    assert manager.record_path(item["id"]).exists()


def test_ambiguous_successful_deletion_is_reconciled_once(setup):
    manager, host, repo, _ = setup
    item = add(setup, age=IDLE_SECONDS)
    host.ambiguous_delete = True
    result = manager.inspect(repo, apply=True, measure=False)
    assert not result["errors"] and not host.entries
    assert sum(c[1] == "delete" for c in host.calls) == 1
    assert not manager.record_path(item["id"]).exists()


def test_final_activity_recheck_catches_new_guest_work(setup, monkeypatch):
    manager, host, repo, _ = setup
    add(setup, age=IDLE_SECONDS)
    calls = iter([True, False])
    monkeypatch.setattr(manager, "idle", lambda _: next(calls))
    result = manager.inspect(repo, apply=True, measure=False)
    assert result["errors"] and host.entries
    assert not any(c[1] == "stop" for c in host.calls)


def test_image_age_digest_current_recipe_and_unknown_references(setup):
    manager, host, repo, now = setup
    old = add(setup, recipe="a" * 12)
    old_ref = old["configuration"]["image"]["reference"]
    host.entries.clear()
    now[0] += IDLE_SECONDS
    # An unmanaged alias reference protects the same digest.
    alias = copy.deepcopy(old)
    alias["configuration"]["labels"] = {}
    alias["configuration"]["image"]["reference"] = "keep:alias"
    host.entries.append(alias)
    manager.inspect(repo, apply=True, measure=False)
    assert old_ref in host.images
    host.entries.clear()
    host.images[old_ref] = "sha256:replacement"
    assert manager.inspect(repo, apply=True, measure=False)["errors"]
    host.images[old_ref] = old["configuration"]["image"]["descriptor"]["digest"]
    assert not manager.inspect(repo, apply=True, measure=False)["errors"]
    assert old_ref not in host.images
    assert f"magik-build:{recipe_key(repo)}" in host.images


def test_failed_and_interrupted_builds_touch_and_run_final_cleanup(setup, monkeypatch):
    manager, host, repo, now = setup
    for error in (RuntimeError("compile failed"), KeyboardInterrupt()):
        cleanup = []
        monkeypatch.setattr(
            manager, "automatic_cleanup", lambda _: cleanup.append(True)
        )
        with pytest.raises(type(error)):
            with manager.build_session(repo):
                manager.prepare(repo)
                now[0] += 10
                raise error
        assert len(cleanup) == 2
        assert manager.last_used(host.entries[0]) == now[0]


def test_cleanup_warning_does_not_mask_build_failure(setup, monkeypatch, capsys):
    manager, host, repo, _ = setup
    host.fail = ["list"]
    with pytest.raises(RuntimeError, match="original failure"):
        with manager.build_session(repo):
            raise RuntimeError("original failure")
    assert "storage warning" in capsys.readouterr().err


def test_build_holds_lease_and_cleans_twice(setup, monkeypatch):
    from magik import build

    manager, host, repo, _ = setup
    package = repo / "magik/agent"
    package.mkdir(parents=True)
    calls = []
    monkeypatch.setattr(storage, "Storage", lambda **_: manager)
    monkeypatch.setattr(manager, "automatic_cleanup", lambda _: calls.append("clean"))

    def checked_build(*args, **kwargs):
        with manager.checkout_lock(repo, blocking=False) as acquired:
            assert not acquired
        return "built artifact"

    monkeypatch.setattr(build, "_ensure_arm_package", checked_build)
    assert build.ensure_arm_package(package) == "built artifact"
    assert calls == ["clean", "clean"]
    assert not any(c[1] == "run" for c in host.calls)


def test_guest_process_parser_fails_closed():
    assert guest_idle("1 0 Ss sleep\n3 1 Z rustc <defunct>\n4 0 R ps\n")
    assert not guest_idle("1 0 Ss sleep\n8 0 S sleep\n9 0 R ps\n")
    assert not guest_idle("1 0 Ss bash\n9 0 R ps\n")
    assert not guest_idle("1 0 Ss .cz-init\n2 1 S sleep\n7 1 S sleep\n9 0 R ps\n")
    with pytest.raises(ValueError):
        guest_idle("")
    with pytest.raises(ValueError):
        guest_idle("unexpected output")


def test_real_process_lease_serializes_checkout_and_survives_waiters(setup):
    manager, host, repo, _ = setup
    item = add(setup, age=IDLE_SECONDS)
    owner = Path(item["configuration"]["labels"][LABEL + "checkout"])
    lock = manager.root / "locks" / f"{storage.checkout_key(owner)}.lock"
    code = "from pathlib import Path; import sys; from magik.storage import file_lock\nwith file_lock(Path(sys.argv[1])):\n print('locked', flush=True)\n sys.stdin.readline()\n"
    env = dict(os.environ, PYTHONPATH=str(Path(storage.__file__).parents[1]))
    child = subprocess.Popen(
        [sys.executable, "-c", code, str(lock)],
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        text=True,
        env=env,
    )
    try:
        assert child.stdout.readline().strip() == "locked"
        with manager.checkout_lock(owner, blocking=False) as acquired:
            assert not acquired
        with manager.checkout_lock(repo, blocking=False) as acquired:
            assert acquired
        result = manager.inspect(repo, apply=True, all_idle=True, measure=False)
        assert not result["errors"] and host.entries
        assert result["containers"][0]["reason"] == "build lease held"
    finally:
        child.kill()
        child.wait(timeout=5)
        child.stdin.close()
        child.stdout.close()
    with manager.checkout_lock(owner, blocking=False) as acquired:
        assert acquired
    assert not manager.inspect(repo, apply=True, all_idle=True, measure=False)["errors"]
    assert not host.entries


def test_cli_json_preview_and_no_device_configuration(setup, monkeypatch, capsys):
    from magik import cli

    manager, host, repo, _ = setup
    add(setup, age=IDLE_SECONDS)
    monkeypatch.delenv("MISTER_IP", raising=False)
    monkeypatch.setattr(storage, "Storage", lambda: manager)
    monkeypatch.setattr(cli, "repository", lambda: repo)
    monkeypatch.setattr(
        cli,
        "create_run",
        lambda *_: pytest.fail("storage should not create a device run"),
    )
    monkeypatch.setattr(sys, "argv", ["scripts/magik", "storage", "report", "--json"])
    assert cli.main() == 0
    result = json.loads(capsys.readouterr().out)
    assert result["containers"][0]["eligible"]
    assert not any(c[1] in {"stop", "delete"} for c in host.calls)
    monkeypatch.setattr(
        sys, "argv", ["scripts/magik", "storage", "clean", "--all-idle", "--apply"]
    )
    assert cli.main() == 0
    assert not host.entries
    capsys.readouterr()
    host.fail = ["list"]
    monkeypatch.setattr(sys, "argv", ["scripts/magik", "storage", "report", "--json"])
    assert cli.main() == 2
    assert json.loads(capsys.readouterr().out)["errors"]


def test_two_recipes_in_same_checkout_are_both_eligible(setup):
    manager, host, repo, _ = setup
    add(setup, "shared", age=IDLE_SECONDS)
    add(setup, "shared", age=IDLE_SECONDS, recipe="b" * 12)
    result = manager.inspect(repo, apply=True, measure=False)
    assert not result["errors"] and not host.entries


def test_different_state_root_cannot_adopt_or_delete_container(setup):
    manager, host, repo, _ = setup
    with manager.build_session(repo):
        name = manager.prepare(repo)
    other = Storage(
        root=manager.root.parent / "other-state",
        runner=host,
        clock=manager.clock,
        data_root=manager.data_root,
    )
    result = other.inspect(repo, apply=True, all_idle=True, measure=False)
    assert host.entries and not result["containers"][0]["eligible"]
    with pytest.raises(RuntimeError, match="ownership mismatch"):
        with other.build_session(repo):
            other.prepare(repo)
    assert host.entries[0]["id"] == name


def test_pending_build_lease_protects_image_before_container_creation(setup):
    manager, host, repo, now = setup
    future = repo.parent / "future"
    (future / "magik/build").mkdir(parents=True)
    (future / "magik/build/Containerfile").write_text("FROM pending\n")
    recipe = recipe_key(future)
    item = add(setup, "future", recipe=recipe)
    host.entries.clear()
    now[0] += IDLE_SECONDS
    with manager.build_session(future):
        result = manager.inspect(repo, apply=True, measure=False)
        assert not result["errors"]
        assert item["configuration"]["image"]["reference"] in host.images
    # A stale unlocked lease does not indefinitely pin its image.
    result = manager.inspect(repo, apply=True, measure=False)
    assert not result["errors"]
    assert item["configuration"]["image"]["reference"] not in host.images


def test_recent_unreferenced_image_retained_until_two_hours(setup):
    manager, host, repo, now = setup
    item = add(setup, recipe="c" * 12)
    host.entries.clear()
    ref = item["configuration"]["image"]["reference"]
    now[0] += IDLE_SECONDS - 1
    assert not manager.inspect(repo, apply=True, measure=False)["errors"]
    assert ref in host.images
    now[0] += 1
    assert not manager.inspect(repo, apply=True, measure=False)["errors"]
    assert ref not in host.images


def test_image_inventory_failure_preserves_partial_cleanup_results(setup):
    manager, host, repo, _ = setup
    item = add(setup, age=IDLE_SECONDS)
    host.fail = ["image", "list"]
    result = manager.inspect(repo, apply=True, measure=False)
    assert result["errors"]
    assert result["containers"][0]["removed"]
    assert not any(e["id"] == item["id"] for e in host.entries)


def test_same_checkout_waiter_blocks_until_owner_releases(setup):
    manager, _, repo, _ = setup
    lock = manager.root / "locks" / f"{storage.checkout_key(repo)}.lock"
    env = dict(os.environ, PYTHONPATH=str(Path(storage.__file__).parents[1]))
    code = "from pathlib import Path; import sys; from magik.storage import file_lock\nprint('waiting', flush=True)\nwith file_lock(Path(sys.argv[1])):\n print('acquired', flush=True)\n"
    with manager.checkout_lock(repo):
        child = subprocess.Popen(
            [sys.executable, "-c", code, str(lock)],
            stdout=subprocess.PIPE,
            text=True,
            env=env,
        )
        assert child.stdout.readline().strip() == "waiting"
        assert child.poll() is None
    try:
        output, _ = child.communicate(timeout=5)
        assert output.strip() == "acquired" and child.returncode == 0
    finally:
        if child.poll() is None:
            child.kill()
            child.wait(timeout=5)


def test_preview_includes_images_freed_by_planned_container_removal(setup):
    manager, host, repo, now = setup
    item = add(setup, recipe="d" * 12)
    now[0] += IDLE_SECONDS
    preview = manager.inspect(repo, measure=False)
    candidate = next(
        r
        for r in preview["images"]
        if r["reference"] == item["configuration"]["image"]["reference"]
    )
    assert candidate["eligible"] and not candidate["removed"] and host.entries
    applied = manager.inspect(repo, apply=True, measure=False)
    candidate = next(
        r for r in applied["images"] if r["reference"] == candidate["reference"]
    )
    assert candidate["removed"] and not applied["errors"]
