"""Verified immutable support releases shared by this host's devices."""

from __future__ import annotations

from contextlib import contextmanager
import fcntl
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import zipfile

from .apps import repository
from .token_store import state_root

REPOSITORY = "NigelBreslaw/MiSTer-MagiK"


@contextmanager
def lock(path):
    path.parent.mkdir(parents=True, exist_ok=True)
    with path.open("a") as stream:
        fcntl.flock(stream, fcntl.LOCK_EX)
        yield


def atomic_json(path, value):
    path.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.NamedTemporaryFile(mode="w", dir=path.parent, delete=False) as stream:
        temporary = Path(stream.name)
        try:
            json.dump(value, stream, indent=2)
            stream.flush()
            os.fsync(stream.fileno())
            os.replace(temporary, path)
            descriptor = os.open(path.parent, os.O_RDONLY)
            try:
                os.fsync(descriptor)
            finally:
                os.close(descriptor)
        finally:
            temporary.unlink(missing_ok=True)


def root():
    return state_root() / "updates"


def contracts():
    sys.path.insert(0, str(repository()))
    from scripts.magik_ci import bundle, databases

    return bundle, databases


def selectors():
    path = repository() / "scripts/release/databases/select-published-release.py"
    spec = importlib.util.spec_from_file_location("published_releases", path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def names(kind, version):
    if kind == "platform":
        return f"mister-magik-platform-v0.{version}.zip", "platform-bundle-v0.2.json"
    return f"mister-magik-game-databases-v{version}.zip", "game-databases-manifest.json"


def verify(kind, release):
    directory = Path(release["directory"])
    archive, manifest = names(kind, release["version"])
    checks = {}
    for line in (directory / "SHA256SUMS").read_text().splitlines():
        digest, name = line.split("  ", 1)
        checks[name] = digest
    if hashlib.sha256((directory / manifest).read_bytes()).hexdigest() != checks.get(
        manifest
    ):
        raise ValueError(f"release checksum mismatch: {manifest}")
    # Published checksum files describe ZIP members, not the ZIP container.
    with zipfile.ZipFile(directory / archive) as stream:
        members = stream.namelist()
        if len(members) != len(set(members)):
            raise ValueError("duplicate release archive member")
        for name in members:
            if Path(name).is_absolute() or ".." in Path(name).parts:
                raise ValueError("unsafe release archive path")
            if name != "SHA256SUMS" and not name.endswith("/"):
                if hashlib.sha256(stream.read(name)).hexdigest() != checks.get(name):
                    raise ValueError(f"release checksum mismatch: {name}")
        if stream.read("SHA256SUMS") != (directory / "SHA256SUMS").read_bytes():
            raise ValueError("release checksum file differs from archive")
    bundle, databases = contracts()
    if kind == "platform":
        return bundle.verify(
            directory / archive, directory / manifest, release["version"]
        )
    return databases.verify(
        directory / archive,
        directory / manifest,
        directory / "SHA256SUMS",
        release["version"],
    )


def desired():
    path = root() / "desired.json"
    if not path.exists():
        return None
    value = json.loads(path.read_text())
    for kind in ("platform", "databases"):
        verify(kind, value[kind])
    return value


def update():
    with lock(root() / "download.lock"):
        result = subprocess.run(
            [
                "gh",
                "api",
                "--paginate",
                "--slurp",
                f"repos/{REPOSITORY}/releases?per_page=100",
            ],
            check=True,
            capture_output=True,
            text=True,
            timeout=120,
        )
        pages = json.loads(result.stdout)
        releases = (
            [item for page in pages for item in page]
            if pages and isinstance(pages[0], list)
            else pages
        )
        selector = selectors()
        platforms = selector.durable_platforms(releases)
        selected = {
            "platform": platforms[0] if platforms else None,
            "databases": selector.select_game_databases(releases),
        }
        pair = {"repository": REPOSITORY}
        for kind, release in selected.items():
            if release is None:
                raise ValueError(f"no published numbered {kind} release")
            tag = release["tag_name"]
            version = (
                selector.platform_version(tag)
                if kind == "platform"
                else int(tag.removeprefix("game-databases-v"))
            )
            directory = root() / "releases" / REPOSITORY / tag
            entry = {
                "tag": tag,
                "version": version,
                "directory": str(directory.resolve()),
            }
            if directory.exists():
                verify(kind, entry)
                outcome = "cached"
            else:
                directory.parent.mkdir(parents=True, exist_ok=True)
                with tempfile.TemporaryDirectory(
                    dir=directory.parent, prefix="download-"
                ) as temporary:
                    args = [
                        "gh",
                        "release",
                        "download",
                        tag,
                        "--repo",
                        REPOSITORY,
                        "--dir",
                        temporary,
                    ]
                    for name in (*names(kind, version), "SHA256SUMS"):
                        args.extend(["--pattern", name])
                    subprocess.run(args, check=True, timeout=600)
                    verify(kind, {**entry, "directory": temporary})
                    os.rename(temporary, directory)
                outcome = "downloaded"
            pair[kind] = entry
            print(f"{tag}: {outcome}, verified: {directory.resolve()}")
        atomic_json(root() / "desired.json", pair)
        print("Queued for all devices. Next: scripts/magik deploy --attended")
    return 0
