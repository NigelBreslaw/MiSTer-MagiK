# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

"""One fail-closed gate for the bytes installed by ZIP and Downloader."""

from __future__ import annotations

import hashlib
import json
import os
import re
import shutil
import stat
import subprocess
import tempfile
import zipfile
from pathlib import Path, PurePosixPath
from typing import Any
from urllib.parse import urlsplit

from . import manifest
from .common import atomic_write, sha256_file

ROOT = Path(__file__).resolve().parents[2]
RECEIPT = "validated-candidate.json"
EVIDENCE = "delivery-evidence.json"
CHANNELS = ("alpha", "beta", "release")
PUBLIC = manifest.LAYOUTS["public"]
APP = PUBLIC["root"].removeprefix("/media/fat/")
LAUNCHER = "Scripts/MiSTer-MagiK.sh"
LEGACY_HELPER = "Scripts/MiSTer-MagiK.platform-v3.constants.sh"
ARTWORK = {
    f"{APP}/assets/snes/snes-small-v1.rgb565a": "7a76993e7e1b0063832b94e9d2ad588549587cf09a14ac2ced72d349ed12f766",
    f"{APP}/assets/ui/settings-v1.rgb565a": "44d657ff706a49fd8c8999b7c02ea4cdb7e4a8488a54dc68e0b79235dc40e8ec",
}
REQUIRED = {
    LAUNCHER,
    *ARTWORK,
    *(
        value.removeprefix("/media/fat/")
        for name, value in PUBLIC.items()
        if name != "root"
    ),
    *(
        f"{APP}/{name}"
        for name in (
            "platform-v3.manifest",
            "platform-bundle-v0.2.json",
            "release-v1.txt",
            "game-databases-manifest.json",
            "magik-metadata-v1.bin",
            "arcade-updater-index-v1.lz4b",
            "THIRD-PARTY-NOTICES.txt",
            "SOURCE-OFFER.txt",
            "licenses/COMMERCIAL-FONTS.txt",
        )
    ),
}


def canonical_json(value: Any) -> bytes:
    return (json.dumps(value, sort_keys=True, indent=2) + "\n").encode()


def _unique(pairs: list[tuple[str, Any]]) -> dict[str, Any]:
    result: dict[str, Any] = {}
    for key, value in pairs:
        if key in result:
            raise ValueError(f"duplicate JSON key: {key}")
        result[key] = value
    return result


def read_json(path: Path) -> dict[str, Any]:
    value = json.loads(path.read_bytes(), object_pairs_hook=_unique)
    if not isinstance(value, dict):
        raise TypeError(f"expected JSON object: {path.name}")
    return value


def safe_path(value: str) -> str:
    if not value or "\\" in value or ":" in value or any(ord(c) < 32 for c in value):
        raise ValueError(f"unsafe package path: {value!r}")
    path = PurePosixPath(value)
    if path.is_absolute() or any(part in {"", ".", ".."} for part in value.split("/")):
        raise ValueError(f"unsafe package path: {value!r}")
    return value


def asset_name(path: str) -> str:
    return "mister-magik--" + safe_path(path).replace("/", "--")


def _inventory(root: Path) -> dict[str, str]:
    return {
        p.relative_to(root).as_posix(): sha256_file(p)
        for p in sorted(root.rglob("*"))
        if p.is_file()
    }


def write_checksums(candidate: Path) -> None:
    atomic_write(
        candidate / "SHA256SUMS",
        "".join(
            f"{sha256_file(path)}  {path.name}\n"
            for path in sorted(candidate.iterdir())
            if path.name != "SHA256SUMS"
        ).encode(),
    )


def verify_checksums(candidate: Path) -> None:
    expected = {}
    for line in (candidate / "SHA256SUMS").read_text().splitlines():
        digest, name = line.split("  ", 1)
        if (
            safe_path(name) != Path(name).name
            or name in expected
            or not re.fullmatch(r"[0-9a-f]{64}", digest)
        ):
            raise ValueError("invalid transport checksum entry")
        expected[name] = digest
    actual = {}
    for path in candidate.iterdir():
        if path.is_symlink() or not path.is_file():
            raise ValueError("candidate must contain only flat regular files")
        if path.name != "SHA256SUMS":
            actual[path.name] = sha256_file(path)
    if actual != expected:
        raise ValueError("candidate transport checksum mismatch")


def extract_package(archive_path: Path, destination: Path) -> dict[str, int]:
    modes = {}
    seen = set()
    with zipfile.ZipFile(archive_path) as archive:
        for entry in archive.infolist():
            name = safe_path(
                entry.filename.rstrip("/") if entry.is_dir() else entry.filename
            )
            if name.casefold() in seen:
                raise ValueError(f"duplicate/case-colliding ZIP entry: {name}")
            seen.add(name.casefold())
            mode = entry.external_attr >> 16
            if stat.S_IFMT(mode) not in (0, stat.S_IFREG, stat.S_IFDIR) or (
                stat.S_ISDIR(mode) and not entry.is_dir()
            ):
                raise ValueError(f"non-regular ZIP entry: {name}")
            target = destination / name
            if entry.is_dir():
                target.mkdir(parents=True, exist_ok=True)
                continue
            target.parent.mkdir(parents=True, exist_ok=True)
            with archive.open(entry) as source, target.open("wb") as output:
                shutil.copyfileobj(source, output)
            target.chmod(0o755 if mode & 0o111 else 0o644)
            modes[name] = mode
    return modes


def verify_root(root: Path) -> dict[str, str]:
    names = set(_inventory(root))
    missing = REQUIRED - names
    if missing:
        raise ValueError(f"missing package files: {', '.join(sorted(missing))}")
    scripts = {
        name
        for name in names
        if name.casefold().startswith("Scripts/MiSTer-MagiK".casefold())
    }
    if scripts != {LAUNCHER}:
        raise ValueError("package must expose exactly one MagiK Scripts entry")
    for name in names:
        if (
            not (
                name == LAUNCHER
                or name == PUBLIC["main"].removeprefix("/media/fat/")
                or name.startswith(APP + "/")
            )
            or "mister-magik-agent" in name
            or "mister-magik-dev" in name
            or "/experiments/" in name
            or name.endswith(("/mame.sqlite3", "/hbmame.sqlite3"))
        ):
            raise ValueError(f"forbidden public payload: {name}")
    for name, expected in ARTWORK.items():
        if sha256_file(root / name) != expected:
            raise ValueError(f"artwork hash mismatch: {name}")
    fields = manifest.verify(root / APP / "platform-v3.manifest", root, layout="public")
    manager = Path(
        os.environ.get(
            "MISTER_MAGIK_HOST_MANAGER",
            str(ROOT / "mister/tools/manager/target/debug/mister-magik-manager"),
        )
    )
    if not manager.is_file():
        raise FileNotFoundError(f"host manager missing: {manager}")
    result = subprocess.run(
        [str(manager), "verify-platform"],
        env={
            **os.environ,
            "MISTER_MAGIK_FAT": str(root),
            "MISTER_MAGIK_INITTAB": str(root / "test-inittab"),
            "MISTER_MAGIK_TEST_MODE": "1",
        },
        capture_output=True,
        text=True,
        timeout=60,
        check=False,
    )
    if result.returncode:
        raise ValueError(
            f"manager rejected package: {result.stdout.strip()} {result.stderr.strip()}"
        )
    return fields


def reconstruct(
    candidate: Path, channel: str, receipt: dict[str, Any], destination: Path
) -> dict[str, Any]:
    database_name = f"mister-magik-{channel}-db.json"
    database = read_json(candidate / database_name)
    with zipfile.ZipFile(candidate / (database_name + ".zip")) as zipped:
        if (
            zipped.namelist() != [database_name]
            or zipped.read(database_name) != (candidate / database_name).read_bytes()
        ):
            raise ValueError("compressed Downloader database mismatch")
    if (
        database.get("db_id") != "mister_magik"
        or database.get("v") != 1
        or database.get("release")
        != {"version": receipt["version"], "build_number": receipt["build_number"]}
    ):
        raise ValueError("Downloader release identity mismatch")
    entries = {entry["path"]: entry for entry in receipt["files"]}
    if set(entries) != set(database["files"]):
        raise ValueError("Downloader/receipt file set mismatch")
    repositories = set()
    for name, entry in entries.items():
        safe_path(name)
        item = database["files"][name]
        url = urlsplit(item["url"])
        match = re.fullmatch(
            r"/([\w.-]+/[\w.-]+)/releases/download/([^/]+)/([^/]+)", url.path
        )
        if (
            url.scheme != "https"
            or url.netloc != "github.com"
            or url.query
            or url.fragment
            or not match
            or match[3] != entry["asset"]
        ):
            raise ValueError(f"invalid Downloader asset URL: {name}")
        # The immutable-only restriction is tightened together with publication.
        if match[2] not in (channel, "v" + receipt["version"]):
            raise ValueError("Downloader tag mismatch")
        repositories.add(match[1])
        source = candidate / entry["asset"]
        if item["size"] != entry["size"] or item["hash"] != entry["md5"]:
            raise ValueError(f"Downloader asset identity mismatch: {name}")
        target = destination / name
        target.parent.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(source, target)
    if len(repositories) != 1:
        raise ValueError("mixed Downloader repositories")
    expected_folders = {
        parent.as_posix() + "/"
        for name in entries
        for parent in PurePosixPath(name).parents
        if parent != PurePosixPath(".")
    }
    if database.get("folders") != {folder: {} for folder in expected_folders}:
        raise ValueError("Downloader folder set mismatch")
    installer = candidate / f"mister-magik-{channel}-installer.zip"
    if channel == "alpha":
        if installer.exists():
            raise ValueError("alpha must not publish an installer drop-in")
    else:
        repository = next(iter(repositories))
        expected_ini = f"[mister_magik]\ndb_url = https://raw.githubusercontent.com/{repository}/downloader/mister-magik-{channel}-db.json.zip\n"
        with zipfile.ZipFile(installer) as zipped:
            if (
                zipped.namelist() != ["downloader_mister_magik.ini"]
                or zipped.read("downloader_mister_magik.ini").decode() != expected_ini
            ):
                raise ValueError("installer drop-in mismatch")
    return database


def verify(
    candidate: Path, *, channel: str, write_receipt: bool = False
) -> dict[str, Any]:
    if channel not in CHANNELS:
        raise ValueError("invalid distribution channel")
    candidate = candidate.resolve()
    verify_checksums(candidate)
    receipt = read_json(candidate / "release-assets.json")
    build = receipt["build_number"]
    version = receipt["version"]
    if (
        receipt.get("format") != "mister-magik-release-assets-v1"
        or type(build) is not int
        or build < 1
        or version != f"0.2.{build}"
        or receipt["archive"] != f"mister-magik-{version}.zip"
    ):
        raise ValueError("invalid release receipt")
    archive = candidate / receipt["archive"]
    if sha256_file(archive) != receipt["archive_sha256"]:
        raise ValueError("archive receipt hash mismatch")
    entries = receipt["files"]
    if len({entry["path"].casefold() for entry in entries}) != len(entries) or len(
        {entry["asset"] for entry in entries}
    ) != len(entries):
        raise ValueError("duplicate/case-colliding receipt entry")
    hashes = {}
    for entry in entries:
        name = safe_path(entry["path"])
        if entry["asset"] != asset_name(name):
            raise ValueError("noncanonical release asset name")
        source = candidate / entry["asset"]
        if (
            source.stat().st_size != entry["size"]
            or sha256_file(source) != entry["sha256"]
            or hashlib.md5(source.read_bytes()).hexdigest() != entry["md5"]
        ):
            raise ValueError(f"release asset receipt mismatch: {name}")
        hashes[name] = entry["sha256"]
    allowed = {entry["asset"] for entry in entries} | {
        receipt["archive"],
        "release-assets.json",
        "SHA256SUMS",
        RECEIPT,
        EVIDENCE,
    }
    present_channels = [
        value
        for value in CHANNELS
        if (candidate / f"mister-magik-{value}-db.json").exists()
    ]
    if channel not in present_channels:
        raise ValueError("requested channel database missing")
    for value in present_channels:
        allowed.update(
            {f"mister-magik-{value}-db.json", f"mister-magik-{value}-db.json.zip"}
        )
        if value != "alpha":
            allowed.add(f"mister-magik-{value}-installer.zip")
    if {path.name for path in candidate.iterdir()} - allowed:
        raise ValueError("unexpected candidate asset")
    with tempfile.TemporaryDirectory(prefix="magik-distribution-") as temporary:
        root = Path(temporary)
        zip_root = root / "zip"
        modes = extract_package(archive, zip_root)
        if _inventory(zip_root) != hashes:
            raise ValueError("ZIP/receipt payload mismatch")
        for name in (
            LAUNCHER,
            PUBLIC["main"].removeprefix("/media/fat/"),
            PUBLIC["gui"].removeprefix("/media/fat/"),
            PUBLIC["manager"].removeprefix("/media/fat/"),
        ):
            if not modes.get(name, 0) & 0o111:
                raise ValueError(f"nonexecutable packaged program: {name}")
        fields = verify_root(zip_root)
        for value in present_channels:
            downloaded = root / value
            reconstruct(candidate, value, receipt, downloaded)
            if _inventory(downloaded) != hashes:
                raise ValueError("ZIP/Downloader payload mismatch")
            verify_root(downloaded)
        release = manifest.parse_fields((zip_root / APP / "release-v1.txt").read_text())
        database = read_json(zip_root / APP / "game-databases-manifest.json")
        platform = read_json(zip_root / APP / "platform-bundle-v0.2.json")
        if (
            release.get("version") != version
            or release.get("build_number") != str(build)
            or release.get("magik_revision") != fields["magik_revision"]
            or release.get("main_revision") != fields["main_revision"]
            or release.get("main_sha256") != fields["main_sha256"]
            or release.get("platform_bundle_id") != fields["platform_bundle_id"]
            or platform.get("bundle_id") != fields["platform_bundle_id"]
            or platform.get("release_version") != int(fields["platform_release_number"])
            or str(database.get("release_version"))
            != release.get("game_database_version")
        ):
            raise ValueError("package release identities disagree")
        if (
            version.encode()
            not in (zip_root / PUBLIC["gui"].removeprefix("/media/fat/")).read_bytes()
        ):
            raise ValueError("GUI embedded release version mismatch")
    validated = {
        "format": "mister-magik-validated-candidate-v1",
        "version": version,
        "build_number": build,
        "source_revision": fields["magik_revision"],
        "platform_bundle_id": fields["platform_bundle_id"],
        "platform_candidate_id": fields["qualification_candidate_id"],
        "game_database_sha256": hashes[f"{APP}/game-databases-manifest.json"],
        "assets": {
            name: sha256_file(candidate / name)
            for name in sorted(
                {entry["asset"] for entry in entries}
                | {receipt["archive"], "release-assets.json"}
            )
        },
        "validation": "passed",
    }
    validated["candidate_id"] = hashlib.sha256(canonical_json(validated)).hexdigest()
    destination = candidate / RECEIPT
    if destination.exists():
        if destination.read_bytes() != canonical_json(validated):
            raise ValueError("validated candidate changed")
    elif write_receipt:
        atomic_write(destination, canonical_json(validated))
        write_checksums(candidate)
    else:
        raise ValueError(
            "validated candidate receipt missing; validate with --write-receipt first"
        )
    return validated
