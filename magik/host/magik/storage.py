"""Host-only Apple Container ownership, build leases and bounded idle retention."""

from __future__ import annotations

import fcntl
import hashlib
import json
import math
import os
import re
import shutil
import subprocess
import sys
import tempfile
import time
from contextlib import contextmanager, ExitStack
from pathlib import Path

from .token_store import state_root

IDLE_SECONDS = 2 * 60 * 60
MAX_IDLE = 4
LABEL = "io.mister-magik.build."
VERSION = "1"


def checkout_key(repository: Path) -> str:
    return hashlib.sha256(str(repository.resolve()).encode()).hexdigest()[:12]


def recipe_key(repository: Path) -> str:
    return hashlib.sha256(
        (repository / "magik/build/Containerfile").read_bytes()
    ).hexdigest()[:12]


def container_name(repository: Path, recipe: str) -> str:
    # A new namespace avoids silently adopting legacy containers without leases.
    return f"magik-v1-{checkout_key(repository)}-{recipe}"


@contextmanager
def file_lock(path: Path, *, blocking: bool = True):
    """Never unlink lock files: waiters must continue to lock the same inode."""
    path.parent.mkdir(parents=True, exist_ok=True)
    with path.open("a+") as handle:
        try:
            fcntl.flock(handle, fcntl.LOCK_EX | (0 if blocking else fcntl.LOCK_NB))
        except BlockingIOError:
            yield False
            return
        try:
            yield True
        finally:
            fcntl.flock(handle, fcntl.LOCK_UN)


def atomic_json(path: Path, value: dict) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.NamedTemporaryFile(mode="w", dir=path.parent, delete=False) as output:
        temporary = Path(output.name)
        json.dump(value, output)
    try:
        temporary.replace(path)
    finally:
        temporary.unlink(missing_ok=True)


def allocation(path: Path) -> dict:
    """Per-file accounting, not an estimate of uniquely freeable APFS blocks."""
    logical = allocated = 0

    def raise_error(error):
        raise error

    for directory, _, files in os.walk(path, onerror=raise_error):
        for filename in files:
            stat = (Path(directory) / filename).stat()
            logical += stat.st_size
            allocated += stat.st_blocks * 512
    return {"logical_bytes": logical, "allocated_bytes": allocated}


def guest_idle(output: str) -> bool:
    """Only the known init/sleeper tree, this inspection and zombies are idle."""
    rows = [line.split(None, 3) for line in output.splitlines() if line.strip()]
    if not rows or any(len(row) != 4 for row in rows):
        raise ValueError("unrecognized guest process inventory")
    processes = [
        (int(pid), int(ppid), state, command) for pid, ppid, state, command in rows
    ]
    roots = {pid: command for pid, _, _, command in processes}
    if roots.get(1) not in {"sleep", ".cz-init"}:
        return False
    for pid, ppid, state, command in processes:
        if state.startswith("Z"):
            continue
        if command == "ps" and ppid == 0:
            continue
        if pid == 1 and command in {"sleep", ".cz-init"}:
            continue
        if command == "sleep" and pid == 2 and ppid == 1 and roots.get(1) == ".cz-init":
            continue
        return False
    return True


class Storage:
    def __init__(
        self,
        *,
        root: Path | None = None,
        runner=subprocess.run,
        clock=time.time,
        data_root: Path | None = None,
    ):
        self.root = root if root is not None else state_root() / "build-storage"
        self.runner = runner
        self.clock = clock
        self.data_root = (
            data_root or Path.home() / "Library/Application Support/com.apple.container"
        )

    def lifecycle(self):
        return file_lock(self.root / "lifecycle.lock")

    def checkout_lock(self, repository: Path, *, blocking: bool = True):
        return file_lock(
            self.root / "locks" / f"{checkout_key(repository)}.lock", blocking=blocking
        )

    def command(self, *args: str):
        return self.runner(
            ["container", *args], check=True, capture_output=True, text=True, timeout=30
        )

    def containers(self) -> list[dict]:
        entries = json.loads(self.command("list", "--all", "--format", "json").stdout)
        if not isinstance(entries, list):
            raise ValueError("invalid container inventory")
        return entries

    def images(self) -> dict[str, str]:
        entries = json.loads(self.command("image", "list", "--format", "json").stdout)
        return {
            entry["configuration"]["name"].removeprefix("docker.io/library/"): entry[
                "configuration"
            ]["descriptor"]["digest"]
            for entry in entries
        }

    def owned(self, entry: dict) -> tuple[Path, str] | None:
        config = entry["configuration"]
        labels = config.get("labels", {})
        if labels.get(LABEL + "version") != VERSION:
            return None
        if labels.get(LABEL + "state") != str(self.root.resolve()):
            return None
        repository = Path(labels[LABEL + "checkout"])
        recipe = labels[LABEL + "recipe"]
        if (
            not repository.is_absolute()
            or str(repository.resolve()) != str(repository)
            or not re.fullmatch(r"[0-9a-f]{12}", recipe)
            or entry["id"] != container_name(repository, recipe)
            or config["image"]["reference"].removeprefix("docker.io/library/")
            != f"magik-build:{recipe}"
            or not any(
                m["destination"] == "/workspace" and m["source"] == str(repository)
                for m in config["mounts"]
            )
        ):
            raise ValueError(f"invalid ownership for {entry['id']}")
        return repository, recipe

    def record_path(self, name: str) -> Path:
        if not re.fullmatch(r"magik-v1-[0-9a-f]{12}-[0-9a-f]{12}", name):
            raise ValueError("invalid managed container identity")
        return self.root / "containers" / f"{name}.json"

    def last_used(self, entry: dict) -> float:
        record = json.loads(self.record_path(entry["id"]).read_text())
        if (
            record["id"] != entry["id"]
            or record["creation_date"] != entry["configuration"]["creationDate"]
            or record["version"] != VERSION
        ):
            raise ValueError("container metadata identity mismatch")
        return self.timestamp(record["last_used"])

    def timestamp(self, value) -> float:
        if (
            isinstance(value, bool)
            or not isinstance(value, (int, float))
            or not math.isfinite(value)
            or value < 0
            or value > self.clock()
        ):
            raise ValueError("invalid last-use timestamp")
        return float(value)

    def touch(self, entry: dict) -> None:
        ownership = self.owned(entry)
        if ownership is None:
            raise ValueError("cannot record an unmanaged container")
        _, recipe = ownership
        now = self.clock()
        atomic_json(
            self.record_path(entry["id"]),
            {
                "version": VERSION,
                "id": entry["id"],
                "creation_date": entry["configuration"]["creationDate"],
                "last_used": now,
            },
        )
        atomic_json(
            self.root / "images" / f"{recipe}.json",
            {
                "version": VERSION,
                "reference": f"magik-build:{recipe}",
                "digest": entry["configuration"]["image"]["descriptor"]["digest"],
                "last_used": now,
            },
        )

    def idle(self, entry: dict) -> bool:
        state = entry["status"]["state"]
        if state == "stopped":
            return True
        if state != "running":
            raise ValueError(f"uncertain container state: {state}")
        return guest_idle(
            self.command(
                "exec", entry["id"], "ps", "-eo", "pid=,ppid=,stat=,comm="
            ).stdout
        )

    def prepare(self, repository: Path) -> str:
        """Called with the checkout lease held; image creation shares the lifecycle lock."""
        repository = repository.resolve()
        recipe = recipe_key(repository)
        name = container_name(repository, recipe)
        image = f"magik-build:{recipe}"
        with self.lifecycle():
            existing = next((e for e in self.containers() if e["id"] == name), None)
            if existing:
                if self.owned(existing) != (repository, recipe):
                    raise RuntimeError("build container ownership mismatch")
                if not self.idle(existing):
                    raise RuntimeError(
                        "build container still has an active guest process"
                    )
                if existing["status"]["state"] == "stopped":
                    self.command("start", name)
            else:
                if image not in self.images():
                    self.runner(
                        [
                            "container",
                            "build",
                            "--tag",
                            image,
                            "--file",
                            str(repository / "magik/build/Containerfile"),
                            str(repository / "magik/build"),
                        ],
                        check=True,
                    )
                cache = Path(
                    os.environ.get(
                        "MISTER_MAGIK2_BUILD_CACHE",
                        str(Path.home() / ".cache/mister-magik/cargo"),
                    )
                ).expanduser()
                mounts = ["--volume", f"{repository}:/workspace"]
                for component in ("registry", "git"):
                    path = cache / component
                    path.mkdir(parents=True, exist_ok=True)
                    mounts += ["--volume", f"{path}:/root/.cargo/{component}"]
                self.command(
                    "run",
                    "--detach",
                    "--init",
                    "--name",
                    name,
                    "--label",
                    f"{LABEL}version={VERSION}",
                    "--label",
                    f"{LABEL}state={self.root.resolve()}",
                    "--label",
                    f"{LABEL}checkout={repository}",
                    "--label",
                    f"{LABEL}recipe={recipe}",
                    "--cpus",
                    "4",
                    "--memory",
                    "4g",
                    *mounts,
                    image,
                    "sleep",
                    "infinity",
                )
            entry = next(e for e in self.containers() if e["id"] == name)
            self.touch(entry)
        return name

    @contextmanager
    def build_session(self, repository: Path):
        repository = repository.resolve()
        try:
            with self.checkout_lock(repository):
                with self.lifecycle():
                    atomic_json(
                        self.root / "leases" / f"{checkout_key(repository)}.json",
                        {
                            "repository": str(repository),
                            "recipe": recipe_key(repository),
                        },
                    )
                self.automatic_cleanup(repository)
                try:
                    yield self
                finally:
                    try:
                        with self.lifecycle():
                            for entry in self.containers():
                                if entry["id"] == container_name(
                                    repository, recipe_key(repository)
                                ):
                                    self.touch(entry)
                    except Exception as error:
                        self.warn(error)
        finally:
            # Include failure/interrupt paths, after releasing the build lease.
            self.automatic_cleanup(repository)

    @staticmethod
    def warn(error) -> None:
        print(f"magik storage warning: {error}", file=sys.stderr)

    def automatic_cleanup(self, repository: Path) -> None:
        try:
            result = self.inspect(repository, apply=True, measure=False)
            for error in result["errors"]:
                self.warn(error)
        except Exception as error:
            self.warn(error)

    def delete_container(self, entry: dict) -> None:
        """No blind mutation retries; uncertain results are reconciled then reported."""
        name = entry["id"]
        try:
            if entry["status"]["state"] == "running":
                self.command("stop", name)
            self.command("delete", name)
        except Exception as error:
            remaining = self.containers()
            if any(e["id"] == name for e in remaining):
                raise RuntimeError(
                    f"{name}: deletion incomplete; inspect before retrying: {error}"
                ) from error
        if any(e["id"] == name for e in self.containers()):
            raise RuntimeError(f"{name}: still present after deletion")
        self.record_path(name).unlink(missing_ok=True)

    def inspect(
        self,
        repository: Path,
        *,
        apply: bool = False,
        all_idle: bool = False,
        measure: bool = True,
    ) -> dict:
        result = {
            "idle_seconds": IDLE_SECONDS,
            "max_idle": MAX_IDLE,
            "apply": apply,
            "containers": [],
            "images": [],
            "errors": [],
        }
        before = shutil.disk_usage(self.data_root).free if measure else None
        with self.lifecycle(), ExitStack() as leases:
            entries = self.containers()
            idle = []
            locked = {}
            for entry in entries:
                row = {
                    "id": entry["id"],
                    "ownership": "unmanaged",
                    "activity": "unknown",
                    "workspace": None,
                    "workspace_exists": None,
                    "last_used": None,
                    "eligible": False,
                    "reason": "unmanaged",
                    "removed": False,
                }
                result["containers"].append(row)
                try:
                    config = entry["configuration"]
                    workspace = next(
                        (
                            m["source"]
                            for m in config["mounts"]
                            if m["destination"] == "/workspace"
                        ),
                        None,
                    )
                    row.update(
                        workspace=workspace,
                        workspace_exists=Path(workspace).exists()
                        if workspace
                        else None,
                    )
                    if measure and re.fullmatch(r"[A-Za-z0-9_.-]+", entry["id"]):
                        row.update(
                            allocation(self.data_root / "containers" / entry["id"])
                        )
                    ownership = self.owned(entry)
                    if ownership is None:
                        if LABEL + "version" in config.get("labels", {}):
                            row.update(
                                ownership="other-manager",
                                reason="different management version or state root",
                            )
                        elif entry["id"].startswith("magik-"):
                            row.update(
                                ownership="migration-candidate",
                                reason="unlabeled legacy container",
                            )
                        row["activity"] = entry["status"]["state"]
                        continue
                    row["ownership"] = "managed"
                    owner, _ = ownership
                    if owner not in locked:
                        locked[owner] = leases.enter_context(
                            self.checkout_lock(owner, blocking=False)
                        )
                    if not locked[owner]:
                        row.update(activity="active", reason="build lease held")
                        continue
                    row["last_used"] = self.last_used(entry)
                    if not self.idle(entry):
                        row.update(activity="active", reason="guest workload")
                        continue
                    row.update(activity="idle", reason="retained for reuse")
                    idle.append((row, entry))
                except Exception as error:
                    row.update(activity="unknown", reason=str(error))
                    result["errors"].append(f"{entry['id']}: {error}")
            idle.sort(key=lambda pair: (pair[0]["last_used"], pair[0]["id"]))
            retained = []
            for row, entry in idle:
                if (
                    all_idle
                    or not row["workspace_exists"]
                    or self.clock() - row["last_used"] >= IDLE_SECONDS
                ):
                    row.update(
                        eligible=True,
                        reason="all idle"
                        if all_idle
                        else "checkout missing"
                        if not row["workspace_exists"]
                        else "idle expiry",
                    )
                else:
                    retained.append((row, entry))
            for row, _ in retained[: max(0, len(retained) - MAX_IDLE)]:
                row.update(eligible=True, reason="idle count limit")
            if apply:
                for row, entry in idle:
                    if not row["eligible"]:
                        continue
                    try:
                        # Locks remain held through this final inventory and guest check.
                        fresh = next(
                            (e for e in self.containers() if e["id"] == entry["id"]),
                            None,
                        )
                        if (
                            fresh is None
                            or fresh["configuration"] != entry["configuration"]
                            or not self.idle(fresh)
                        ):
                            raise RuntimeError(
                                "container changed or became active; skipped"
                            )
                        self.delete_container(fresh)
                        row["removed"] = True
                    except Exception as error:
                        result["errors"].append(f"{entry['id']}: {error}")
            # Re-inventory after deletions; even unknown containers protect their images.
            planned_removals = {row["id"] for row, _ in idle if row["eligible"]}
            current = (
                self.containers()
                if apply
                else [entry for entry in entries if entry["id"] not in planned_removals]
            )
            try:
                self.inspect_images(
                    repository, current, result, apply=apply, checkout_locks=locked
                )
            except Exception as error:
                result["errors"].append(f"image inspection: {error}")
            if measure:
                try:
                    result["disk_usage"] = json.loads(
                        self.command("system", "df", "--format", "json").stdout
                    )
                    result["accounting_path"] = str(self.data_root)
                    result["allocation"] = allocation(self.data_root)
                except Exception as error:
                    result["errors"].append(f"disk accounting: {error}")
        if measure:
            result["free_bytes_before"] = before
            result["free_bytes_after"] = shutil.disk_usage(self.data_root).free
            result["free_bytes_change"] = result["free_bytes_after"] - before
        return result

    def inspect_images(
        self,
        repository: Path,
        entries: list,
        result: dict,
        *,
        apply: bool,
        checkout_locks: dict,
    ):
        images = self.images()
        protected = {
            e["configuration"]["image"]["descriptor"]["digest"] for e in entries
        }
        image_rows = {
            ref: {
                "reference": ref,
                "digest": digest,
                "ownership": "unmanaged",
                "last_used": None,
                "eligible": False,
                "removed": False,
                "reason": "not managed by this state root",
            }
            for ref, digest in sorted(images.items())
        }
        result["images"].extend(image_rows.values())
        protected_recipes = {recipe_key(repository)}
        # A build may hold a lease before its container exists (e.g. cache check).
        # Corrupt lease records block image deletion rather than guessing ownership.
        for path in sorted((self.root / "leases").glob("*.json")):
            lease = json.loads(path.read_text())
            owner = Path(lease["repository"])
            if (
                not owner.is_absolute()
                or path.stem != checkout_key(owner)
                or not re.fullmatch(r"[0-9a-f]{12}", lease["recipe"])
            ):
                raise ValueError("invalid build lease metadata")
            if owner in checkout_locks:
                if not checkout_locks[owner]:
                    protected_recipes.add(lease["recipe"])
            else:
                with self.checkout_lock(owner, blocking=False) as acquired:
                    if not acquired:
                        protected_recipes.add(lease["recipe"])
        for path in sorted((self.root / "images").glob("*.json")):
            try:
                record = json.loads(path.read_text())
                ref = record["reference"]
                if (
                    record["version"] != VERSION
                    or not re.fullmatch(r"[0-9a-f]{12}", path.stem)
                    or ref != f"magik-build:{path.stem}"
                ):
                    raise ValueError("invalid managed image record")
                last_used = self.timestamp(record["last_used"])
                if ref not in images:
                    continue
                if images[ref] != record["digest"]:
                    raise ValueError("image digest changed; skipped")
                eligible = (
                    path.stem not in protected_recipes
                    and images[ref] not in protected
                    and self.clock() - last_used >= IDLE_SECONDS
                )
                row = image_rows[ref]
                row.update(
                    ownership="managed",
                    last_used=last_used,
                    eligible=eligible,
                    reason="unused expiry"
                    if eligible
                    else "referenced, current, or recently used",
                )
                if apply and eligible:
                    try:
                        self.command("image", "delete", ref)
                    except Exception:
                        if ref in self.images():
                            raise
                    if ref in self.images():
                        raise RuntimeError("image still present after deletion")
                    row["removed"] = True
                    path.unlink()
            except Exception as error:
                result["errors"].append(f"{path.name}: {error}")


def run_storage(arguments, repository: Path) -> int:
    try:
        result = Storage().inspect(
            repository,
            apply=getattr(arguments, "apply", False),
            all_idle=getattr(arguments, "all_idle", False),
        )
    except Exception as error:
        result = {"containers": [], "images": [], "errors": [str(error)]}
    if getattr(arguments, "json", False):
        print(json.dumps(result, indent=2))
    else:
        print(
            "Apple Container storage (allocated bytes may share APFS blocks; logical bytes are virtual capacity)"
        )
        print(
            "Policy: two hours, four idle containers; cleanup runs during builds, without a background timer."
        )
        for row in result["containers"]:
            print(
                f"{row['id']}: {row['ownership']}, {row['activity']}; {row['reason']}; "
                f"eligible={row['eligible']} removed={row['removed']} allocated={row.get('allocated_bytes', 'unknown')} "
                f"logical={row.get('logical_bytes', 'unknown')} last_used={row['last_used']} "
                f"workspace={row['workspace']} exists={row['workspace_exists']}"
            )
        for row in result["images"]:
            print(
                f"{row['reference']}: {row['ownership']}; {row['reason']}; eligible={row['eligible']} removed={row['removed']}"
            )
        if "free_bytes_change" in result:
            print(
                f"Allocation: {result.get('allocation')}; free-space change: {result['free_bytes_change']} bytes"
            )
        if not getattr(arguments, "apply", False):
            print(
                "Preview only. Use storage clean --apply to remove eligible managed resources."
            )
        for error in result["errors"]:
            print(f"magik storage: {error}", file=sys.stderr)
    return 2 if result["errors"] else 0
