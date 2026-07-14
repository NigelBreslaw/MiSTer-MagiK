#!/usr/bin/env python3
"""Create, verify, and plan numbered MiSTer MagiK game-database bundles."""

from __future__ import annotations

import argparse
import hashlib
import json
import re
import shutil
import sqlite3
import sys
import tempfile
import zipfile
from contextlib import closing
from pathlib import Path, PurePosixPath

FORMAT = "mister-magik-game-databases-manifest-v1"
MANIFEST_NAME = "game-databases-manifest.json"
CHECKSUMS_NAME = "SHA256SUMS"
DATABASES = ("mame.sqlite3", "hbmame.sqlite3")
HEX40 = re.compile(r"[0-9a-f]{40}")
HEX64 = re.compile(r"[0-9a-f]{64}")
MAME_TAG = re.compile(r"mame([0-9]+)")
HBMAME_TAG = re.compile(r"tag([0-9]+)")


class BundleError(ValueError):
    pass


def digest(path: Path) -> str:
    value = hashlib.sha256()
    with path.open("rb") as source:
        for chunk in iter(lambda: source.read(1024 * 1024), b""):
            value.update(chunk)
    return value.hexdigest()


def require_sha(name: str, value: str) -> None:
    if not HEX40.fullmatch(value):
        raise BundleError(f"invalid {name}")


def require_digest(name: str, value: str) -> None:
    if not HEX64.fullmatch(value):
        raise BundleError(f"invalid {name}")


def mame_source_version(tag: str) -> str:
    match = MAME_TAG.fullmatch(tag)
    if not match:
        raise BundleError("invalid MAME tag")
    digits = match.group(1)
    return f"0.{int(digits)} ({tag})"


def validate_database(path: Path, kind: str, mame_tag: str | None = None) -> None:
    if not path.is_file():
        raise BundleError(f"missing {kind} database")
    try:
        with closing(sqlite3.connect(f"file:{path}?mode=ro", uri=True)) as database, database:
            integrity = database.execute("PRAGMA integrity_check").fetchone()
            if not integrity or integrity[0] != "ok":
                raise BundleError(f"{kind} database integrity check failed")
            rows = int(database.execute("SELECT count(*) FROM mame_machines").fetchone()[0])
            minimum = 50_000 if kind == "MAME" else 5_000
            if rows < minimum:
                raise BundleError(f"{kind} database has too few machine rows: {rows}")
            if kind == "MAME":
                if not mame_tag:
                    raise BundleError("MAME tag is required for validation")
                versions = {
                    row[0]
                    for row in database.execute("SELECT DISTINCT source_version FROM mame_machines")
                }
                expected = mame_source_version(mame_tag)
                if versions != {expected}:
                    raise BundleError(f"MAME source version does not match {mame_tag}")
                required = ("megadriv", "n64", "nes", "saturn", "sms", "snes")
                counts = dict(
                    database.execute(
                        "SELECT list_name, count(*) FROM mame_software_items "
                        "WHERE list_name IN (?,?,?,?,?,?) GROUP BY list_name",
                        required,
                    )
                )
                missing = [name for name in required if counts.get(name, 0) == 0]
                if missing:
                    raise BundleError(f"MAME database is missing software lists: {', '.join(missing)}")
            else:
                row = database.execute(
                    "SELECT COALESCE(parent_setname, '') FROM mame_machines WHERE setname='marpy'"
                ).fetchone()
                if not row or row[0] != "mappy":
                    raise BundleError("HBMAME marpy parent sentinel failed")
    except sqlite3.Error as error:
        raise BundleError(f"invalid {kind} SQLite database: {error}") from error


def source_payload(args: argparse.Namespace) -> dict[str, object]:
    if not MAME_TAG.fullmatch(args.mame_tag):
        raise BundleError("invalid MAME tag")
    if not HBMAME_TAG.fullmatch(args.hbmame_tag):
        raise BundleError("invalid HBMAME tag")
    require_sha("MAME SHA", args.mame_sha)
    require_sha("HBMAME SHA", args.hbmame_sha)
    require_sha("MAME builder SHA", args.mame_builder_sha)
    require_sha("HBMAME builder SHA", args.hbmame_builder_sha)
    if not args.mame_listxml_asset or "/" in args.mame_listxml_asset:
        raise BundleError("invalid MAME listxml asset")
    require_digest("MAME listxml SHA-256", args.mame_listxml_sha256)
    return {
        "mame": {
            "tag": args.mame_tag,
            "sha": args.mame_sha,
            "listxml_asset": args.mame_listxml_asset,
            "listxml_sha256": args.mame_listxml_sha256,
            "builder_sha": args.mame_builder_sha,
        },
        "hbmame": {
            "tag": args.hbmame_tag,
            "sha": args.hbmame_sha,
            "builder_sha": args.hbmame_builder_sha,
        },
    }


def write_checksums(root: Path, paths: list[Path]) -> None:
    (root / CHECKSUMS_NAME).write_text(
        "".join(f"{digest(path)}  {path.name}\n" for path in sorted(paths))
    )


def create(args: argparse.Namespace) -> Path:
    if args.release_version < 1:
        raise BundleError("release version must be positive")
    sources = source_payload(args)
    validate_database(args.mame_sqlite, "MAME", args.mame_tag)
    validate_database(args.hbmame_sqlite, "HBMAME")
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=True)
    archive = output / f"mister-magik-game-databases-v{args.release_version}.zip"
    with tempfile.TemporaryDirectory(prefix="mister-magik-game-databases-") as temporary:
        stage = Path(temporary)
        copied: list[Path] = []
        for source, name in ((args.mame_sqlite, DATABASES[0]), (args.hbmame_sqlite, DATABASES[1])):
            destination = stage / name
            shutil.copyfile(source, destination)
            copied.append(destination)
        payload = {
            "format": FORMAT,
            "release_version": args.release_version,
            "sources": sources,
            "files": [
                {"path": path.name, "size": path.stat().st_size, "sha256": digest(path)}
                for path in copied
            ],
        }
        manifest = stage / MANIFEST_NAME
        manifest.write_text(json.dumps(payload, indent=2, sort_keys=True) + "\n")
        write_checksums(stage, [*copied, manifest])
        with zipfile.ZipFile(archive, "w", compression=zipfile.ZIP_DEFLATED) as destination:
            for path in sorted(stage.iterdir()):
                destination.write(path, path.name)
        shutil.copyfile(manifest, output / MANIFEST_NAME)
        shutil.copyfile(stage / CHECKSUMS_NAME, output / CHECKSUMS_NAME)
    verify(archive, output / MANIFEST_NAME)
    return archive


def validate_manifest(payload: dict[str, object]) -> None:
    if payload.get("format") != FORMAT:
        raise BundleError("unsupported game-database manifest format")
    version = payload.get("release_version")
    if not isinstance(version, int) or version < 1:
        raise BundleError("invalid release version")
    sources = payload.get("sources")
    if not isinstance(sources, dict):
        raise BundleError("missing database sources")
    mame = sources.get("mame")
    hbmame = sources.get("hbmame")
    if not isinstance(mame, dict) or not isinstance(hbmame, dict):
        raise BundleError("missing MAME or HBMAME source")
    if not MAME_TAG.fullmatch(str(mame.get("tag", ""))):
        raise BundleError("invalid MAME tag")
    if not HBMAME_TAG.fullmatch(str(hbmame.get("tag", ""))):
        raise BundleError("invalid HBMAME tag")
    require_sha("MAME SHA", str(mame.get("sha", "")))
    require_sha("HBMAME SHA", str(hbmame.get("sha", "")))
    require_sha("MAME builder SHA", str(mame.get("builder_sha", "")))
    require_sha("HBMAME builder SHA", str(hbmame.get("builder_sha", "")))
    asset = str(mame.get("listxml_asset", ""))
    if not asset or "/" in asset:
        raise BundleError("invalid MAME listxml asset")
    require_digest("MAME listxml SHA-256", str(mame.get("listxml_sha256", "")))


def verify(archive: Path, manifest_path: Path | None = None) -> dict[str, object]:
    if not archive.is_file():
        raise BundleError(f"missing bundle archive: {archive}")
    with tempfile.TemporaryDirectory(prefix="mister-magik-game-databases-verify-") as temporary:
        root = Path(temporary)
        with zipfile.ZipFile(archive) as source:
            names: set[str] = set()
            for info in source.infolist():
                member = PurePosixPath(info.filename)
                if member.is_absolute() or ".." in member.parts or len(member.parts) != 1 or info.is_dir():
                    raise BundleError(f"unsafe archive member: {info.filename}")
                if info.filename in names:
                    raise BundleError(f"duplicate archive member: {info.filename}")
                names.add(info.filename)
                source.extract(info, root)
        expected_names = {*DATABASES, MANIFEST_NAME, CHECKSUMS_NAME}
        if names != expected_names:
            raise BundleError("bundle has unexpected or missing files")
        manifest = root / MANIFEST_NAME
        payload = json.loads(manifest.read_text())
        validate_manifest(payload)
        expected_archive = f"mister-magik-game-databases-v{payload['release_version']}.zip"
        if archive.name != expected_archive:
            raise BundleError("archive name does not match release version")
        if manifest_path is not None and json.loads(manifest_path.read_text()) != payload:
            raise BundleError("release manifest differs from archive manifest")
        file_entries = payload.get("files")
        if not isinstance(file_entries, list):
            raise BundleError("manifest files are missing")
        expected_files = {entry["path"]: entry for entry in file_entries if isinstance(entry, dict) and "path" in entry}
        if set(expected_files) != set(DATABASES):
            raise BundleError("manifest database file set is invalid")
        for name in DATABASES:
            path = root / name
            entry = expected_files[name]
            if entry.get("size") != path.stat().st_size or entry.get("sha256") != digest(path):
                raise BundleError(f"manifest does not match {name}")
        checksums: dict[str, str] = {}
        for line in (root / CHECKSUMS_NAME).read_text().splitlines():
            try:
                value, name = line.split("  ", 1)
            except ValueError as error:
                raise BundleError("malformed checksums") from error
            checksums[name] = value
        for name in (*DATABASES, MANIFEST_NAME):
            if checksums.get(name) != digest(root / name):
                raise BundleError(f"checksum mismatch for {name}")
        if set(checksums) != {*DATABASES, MANIFEST_NAME}:
            raise BundleError("checksum file set is invalid")
        mame = payload["sources"]["mame"]
        validate_database(root / DATABASES[0], "MAME", str(mame["tag"]))
        validate_database(root / DATABASES[1], "HBMAME")
        return payload


def verify_files(manifest: Path, mame_sqlite: Path, hbmame_sqlite: Path) -> dict[str, object]:
    payload = json.loads(manifest.read_text())
    validate_manifest(payload)
    entries = payload.get("files")
    if not isinstance(entries, list):
        raise BundleError("manifest files are missing")
    expected = {entry["path"]: entry for entry in entries if isinstance(entry, dict) and "path" in entry}
    paths = {"mame.sqlite3": mame_sqlite, "hbmame.sqlite3": hbmame_sqlite}
    if set(expected) != set(paths):
        raise BundleError("manifest database file set is invalid")
    for name, path in paths.items():
        if not path.is_file() or expected[name].get("size") != path.stat().st_size or expected[name].get("sha256") != digest(path):
            raise BundleError(f"manifest does not match {name}")
    validate_database(mame_sqlite, "MAME", str(payload["sources"]["mame"]["tag"]))
    validate_database(hbmame_sqlite, "HBMAME")
    return payload


def update_plan(
    current: dict[str, object] | None,
    mame_tag: str,
    mame_sha: str,
    hbmame_tag: str,
    hbmame_sha: str,
) -> dict[str, object]:
    mame_source_version(mame_tag)
    if not HBMAME_TAG.fullmatch(hbmame_tag):
        raise BundleError("invalid HBMAME tag")
    require_sha("MAME SHA", mame_sha)
    require_sha("HBMAME SHA", hbmame_sha)
    if current is None:
        return {"current_version": 0, "next_version": 1, "mame_changed": True, "hbmame_changed": True, "update_needed": True}
    validate_manifest(current)
    sources = current["sources"]
    mame_changed = sources["mame"]["tag"] != mame_tag or sources["mame"]["sha"] != mame_sha
    hbmame_changed = sources["hbmame"]["tag"] != hbmame_tag or sources["hbmame"]["sha"] != hbmame_sha
    version = int(current["release_version"])
    return {
        "current_version": version,
        "next_version": version + 1,
        "mame_changed": mame_changed,
        "hbmame_changed": hbmame_changed,
        "update_needed": mame_changed or hbmame_changed,
    }


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    create_parser = commands.add_parser("create")
    create_parser.add_argument("--mame-sqlite", required=True, type=Path)
    create_parser.add_argument("--hbmame-sqlite", required=True, type=Path)
    create_parser.add_argument("--release-version", required=True, type=int)
    create_parser.add_argument("--mame-tag", required=True)
    create_parser.add_argument("--mame-sha", required=True)
    create_parser.add_argument("--mame-listxml-asset", required=True)
    create_parser.add_argument("--mame-listxml-sha256", required=True)
    create_parser.add_argument("--hbmame-tag", required=True)
    create_parser.add_argument("--hbmame-sha", required=True)
    create_parser.add_argument("--mame-builder-sha", required=True)
    create_parser.add_argument("--hbmame-builder-sha", required=True)
    create_parser.add_argument("--output", required=True, type=Path)
    verify_parser = commands.add_parser("verify")
    verify_parser.add_argument("archive", type=Path)
    verify_parser.add_argument("--manifest", type=Path)
    files_parser = commands.add_parser("verify-files")
    files_parser.add_argument("--manifest", required=True, type=Path)
    files_parser.add_argument("--mame-sqlite", required=True, type=Path)
    files_parser.add_argument("--hbmame-sqlite", required=True, type=Path)
    plan_parser = commands.add_parser("plan-update")
    plan_parser.add_argument("--manifest", type=Path)
    plan_parser.add_argument("--mame-tag", required=True)
    plan_parser.add_argument("--mame-sha", required=True)
    plan_parser.add_argument("--hbmame-tag", required=True)
    plan_parser.add_argument("--hbmame-sha", required=True)
    plan_parser.add_argument("--github-output", type=Path)
    args = parser.parse_args()
    try:
        if args.command == "create":
            print(create(args))
        elif args.command == "verify":
            print(json.dumps(verify(args.archive, args.manifest), sort_keys=True))
        elif args.command == "verify-files":
            print(json.dumps(verify_files(args.manifest, args.mame_sqlite, args.hbmame_sqlite), sort_keys=True))
        else:
            current = json.loads(args.manifest.read_text()) if args.manifest else None
            result = update_plan(current, args.mame_tag, args.mame_sha, args.hbmame_tag, args.hbmame_sha)
            if args.github_output:
                with args.github_output.open("a") as output:
                    for key, value in result.items():
                        rendered = str(value).lower() if isinstance(value, bool) else str(value)
                        output.write(f"{key}={rendered}\n")
            print(json.dumps(result, sort_keys=True))
    except (BundleError, OSError, json.JSONDecodeError, sqlite3.Error, zipfile.BadZipFile) as error:
        print(f"game-database bundle invalid: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
