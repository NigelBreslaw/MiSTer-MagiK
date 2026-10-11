# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

"""Read-only upstream Arcade screenshot coverage, independent of installed games."""

from __future__ import annotations

from contextlib import closing

import ast
from concurrent.futures import Future, ThreadPoolExecutor, as_completed
from collections import Counter
import csv
from datetime import datetime, timezone
import hashlib
import html
import io
import json
from pathlib import Path, PurePosixPath
import re
import struct
import sqlite3
import subprocess
from threading import Lock
from urllib.parse import quote, urljoin, urlparse
import zipfile

REGISTRY_URL = "https://raw.githubusercontent.com/theypsilon/Update_All_MiSTer/master/src/update_all/databases.py"
MANIFEST_URL = "https://assets.mistermagik.com/mister-magik/v1/manifest.json"
FORMAT = "mister-magik-upstream-screenshots-v1"


def add_command(commands):
    command = commands.add_parser(
        "upstream-screenshots",
        help="audit all Update All Arcade releases, including beta and alpha",
    )
    command.add_argument(
        "--output", type=Path, default=Path("build/arcade-upstream-audit")
    )
    command.add_argument("--registry-url", default=REGISTRY_URL)
    command.add_argument("--manifest-url", default=MANIFEST_URL)
    command.add_argument(
        "--database-url",
        action="append",
        default=[],
        help="include an additional downloader feed",
    )
    command.add_argument(
        "--mame-listxml",
        type=Path,
        help="use a local MAME listxml ZIP or XML instead of latest release",
    )
    command.add_argument(
        "--baseline",
        type=Path,
        help="compare with a previous audit JSON (default: last complete run)",
    )
    command.add_argument(
        "--hbmame-database",
        type=Path,
        help="use a local HBMAME SQLite parent map instead of the published source artifact",
    )
    command.add_argument("--workers", type=int, choices=range(1, 17), default=8)


def digest(data):
    return hashlib.sha256(data).hexdigest()


class Downloads:
    def __init__(self, cache):
        self.cache = cache
        cache.mkdir(parents=True, exist_ok=True)
        self.api_requests = {}
        self.api_lock = Lock()

    def fetch(self, url, *, immutable=False, limit=100 * 1024 * 1024):
        if urlparse(url).scheme != "https":
            raise ValueError("upstream URL must use HTTPS")
        path = self.cache / digest(url.encode())
        if immutable and path.exists():
            data = path.read_bytes()
        else:
            result = subprocess.run(
                [
                    "curl",
                    "--fail",
                    "--location",
                    "--silent",
                    "--show-error",
                    "--proto",
                    "=https",
                    "--max-time",
                    "60",
                    "--max-filesize",
                    str(limit),
                    url,
                ],
                capture_output=True,
                check=True,
                timeout=65,
            )
            data = result.stdout
            if immutable:
                path.write_bytes(data)
        if len(data) > limit:
            raise ValueError(f"download exceeds {limit} bytes: {url}")
        return data

    def api(self, endpoint):
        # Coalesce shared pinned trees across feeds, including concurrent callers.
        # This cache lasts one audit; mutable API metadata refreshes on the next run.
        with self.api_lock:
            pending = self.api_requests.get(endpoint)
            owner = pending is None
            if owner:
                pending = self.api_requests[endpoint] = Future()
        if owner:
            try:
                result = subprocess.run(
                    ["gh", "api", endpoint], capture_output=True, check=True, timeout=60
                )
                pending.set_result(json.loads(result.stdout))
            except (OSError, ValueError, subprocess.SubprocessError) as error:
                pending.set_exception(error)
        return pending.result()

    def mra(self, row):
        expected = row["hash"]
        if row["bytes"] > 64 * 1024 * 1024:
            raise ValueError("MRA exceeds the 64 MiB inspection limit")
        cache = self.cache / ("mra-" + expected)
        data = (
            cache.read_bytes()
            if cache.exists()
            else self.fetch(row["url"], limit=max(row["bytes"], 4 * 1024 * 1024))
        )
        if row["hash_kind"] == "git":
            actual = hashlib.sha1(
                b"blob " + str(len(data)).encode() + b"\0" + data, usedforsecurity=False
            ).hexdigest()
        else:
            actual = hashlib.md5(data, usedforsecurity=False).hexdigest()
        if actual != expected or len(data) != row["bytes"]:
            raise ValueError(f"MRA checksum/size mismatch: {row['path']}")
        if not cache.exists():
            cache.write_bytes(data)
        return data


def registry_sources(data):
    """Extract literal feed declarations without importing/executing upstream code."""
    text = data.decode("utf-8")
    tree = ast.parse(text)
    constants = {
        node.targets[0].id: node.value.value
        for node in tree.body
        if isinstance(node, ast.Assign)
        and len(node.targets) == 1
        and isinstance(node.targets[0], ast.Name)
        and isinstance(node.value, ast.Constant)
    }
    sections = {}
    section = ""
    for number, line in enumerate(text.splitlines(), 1):
        match = re.match(r"\s+#\s+([A-Z][A-Z /_-]+)\s*$", line)
        if match:
            section = match[1]
        sections[number] = section
    sources = {}
    for cls in tree.body:
        if not isinstance(cls, ast.ClassDef) or cls.name != "AllDBs":
            continue
        for node in ast.walk(cls):
            if not isinstance(node, ast.Assign) or not isinstance(node.value, ast.Call):
                continue
            if (
                not isinstance(node.value.func, ast.Name)
                or node.value.func.id != "Database"
            ):
                continue
            target = node.targets[0]
            if not isinstance(target, ast.Attribute):
                continue
            values = {}
            for item in node.value.keywords:
                if isinstance(item.value, ast.Constant):
                    values[item.arg] = item.value.value
                elif isinstance(item.value, ast.Name) and item.value.id in constants:
                    values[item.arg] = constants[item.value.id]
            label = str(values.get("title", target.attr))
            group = sections.get(node.lineno, "")
            # Core/distribution sections cover providers whose titles omit "core",
            # e.g. Coin-Op Collection and Y/C Builds. Asset/tool sections are not feeds of games.
            selected = bool(re.search(r"CORES|DISTRIBUTION|^JT$", group)) or bool(
                re.search(
                    r"\bcores?\b|distribution|arcade offset|coin.op collection|llapi|y/c builds",
                    label,
                    re.I,
                )
            )
            if not selected:
                continue
            url = values.get("db_url")
            if not isinstance(url, str) or not isinstance(values.get("db_id"), str):
                raise ValueError(
                    f"cannot resolve upstream registry declaration: {target.attr}"
                )
            sources[url] = {
                "id": target.attr.lower(),
                "db_id": values["db_id"],
                "title": label,
                "url": url,
            }
    if not sources:
        raise ValueError(
            "Update All registry contains no recognized core/distribution feeds"
        )
    return sorted(sources.values(), key=lambda source: source["id"])


def decode_database(data):
    if data.startswith(b"PK"):
        with zipfile.ZipFile(io.BytesIO(data)) as archive:
            members = [item for item in archive.infolist() if not item.is_dir()]
            if len(members) != 1 or members[0].file_size > 256 * 1024 * 1024:
                raise ValueError("database ZIP must contain one bounded JSON document")
            data = archive.read(members[0])
    database = json.loads(data)
    if not isinstance(database, dict) or not isinstance(database.get("files"), dict):
        raise ValueError("downloader database has no files map")
    return database


def channel(database, entry, path):
    tags = set(entry.get("tags", []))
    folders = database.get("folders", {})
    for parent in PurePosixPath(path).parents:
        tags.update(folders.get(str(parent), {}).get("tags", []))
    names = {
        re.sub(r"[^a-z0-9]", "", name.lower())
        for name, number in database.get("tag_dictionary", {}).items()
        if number in tags
    }
    if names & {"alpha", "coinopcollectionalpha", "jtalpha"}:
        return "alpha"
    if names & {"beta", "coinopcollectionbeta", "jtbeta"}:
        return "beta"
    return "public"


def source_rows(source, database, downloads):
    rows = []
    known = set()
    base = database.get("base_files_url", "")
    for path, entry in database["files"].items():
        if not path.lower().endswith(".mra"):
            continue
        url = entry.get("url") or urljoin(
            base.rstrip("/") + "/", quote(path.lstrip("/"))
        )
        if not re.fullmatch(
            r"[a-fA-F0-9]{32}", str(entry.get("hash", ""))
        ) or not isinstance(entry.get("size"), int):
            raise ValueError(f"invalid MRA manifest entry: {path}")
        rows.append(
            {
                "source": source["id"],
                "path": path,
                "url": url,
                "hash": entry["hash"].lower(),
                "hash_kind": "md5",
                "bytes": entry["size"],
                "channel": channel(database, entry, path),
            }
        )
        known.add(url)
    # Distribution omits some alternatives from its downloader manifest. Inspect
    # the exact referenced Git tree too; supplements have independently verified Git blob hashes.
    match = re.fullmatch(
        r"https://raw\.githubusercontent\.com/([^/]+/[^/]+)/([a-f0-9]{40})/(.*)", base
    )
    tree_error = None
    if match:
        repo, revision, prefix = match.groups()
        try:
            tree = downloads.api(f"repos/{repo}/git/trees/{revision}?recursive=1")
            if tree.get("truncated"):
                raise ValueError("upstream Git tree is truncated")
            for item in tree["tree"]:
                path = item["path"]
                if (
                    item["type"] != "blob"
                    or not path.lower().endswith(".mra")
                    or not path.startswith(prefix)
                ):
                    continue
                url = (
                    f"https://raw.githubusercontent.com/{repo}/{revision}/{quote(path)}"
                )
                if url in known:
                    continue
                rows.append(
                    {
                        "source": source["id"],
                        "path": path,
                        "url": url,
                        "hash": item["sha"],
                        "hash_kind": "git",
                        "bytes": item["size"],
                        "channel": "unknown",
                    }
                )
        except (OSError, ValueError, KeyError, subprocess.SubprocessError) as error:
            tree_error = error_text(error)
    return rows, tree_error


def pack_keys(data, pack, entries=None):
    index = pack["index"]
    if len(data) != index["bytes"] or digest(data) != index["sha256"]:
        raise ValueError("published screenshot index checksum/size mismatch")
    if len(data) < 84 or data[:8] != b"MMIDX02\0":
        raise ValueError("unsupported screenshot index")
    if (
        struct.unpack_from("<Q", data, 8)[0] != pack["bytes"]
        or data[16:80].decode("ascii") != pack["sha256"]
    ):
        raise ValueError("screenshot index archive identity mismatch")
    if (
        index["archive_bytes"] != pack["bytes"]
        or index["archive_sha256"] != pack["sha256"]
    ):
        raise ValueError("manifest index/archive identity mismatch")
    count = struct.unpack_from("<I", data, 80)[0]
    if count > 100_000:
        raise ValueError("screenshot index entry count exceeds limit")
    keys = set()
    offset = 84
    for _ in range(count):
        if offset + 31 > len(data):
            raise ValueError("truncated screenshot index")
        name_len, width, height, stride, raw, flag, compressed, start = (
            struct.unpack_from("<HIIIIBIQ", data, offset)
        )
        offset += 31
        if offset + name_len > len(data):
            raise ValueError("truncated screenshot key")
        key = data[offset : offset + name_len].decode("utf-8").lower()
        offset += name_len
        if (
            not key
            or not width
            or not height
            or stride < width * 2
            or stride % 2
            or raw != stride * height
        ):
            raise ValueError("invalid/duplicate screenshot index entry")
        if (
            raw > 16 * 1024 * 1024
            or flag not in (0, 1)
            or not compressed
            or start + compressed > pack["bytes"]
        ):
            raise ValueError("invalid screenshot index payload")
        if (flag == 1 and compressed != raw) or (
            flag == 0 and compressed > raw + raw // 255 + 16
        ):
            raise ValueError("invalid screenshot payload length")
        if not key.endswith(".rgb565"):
            raise ValueError("unsupported screenshot entry filename")
        asset_key = key.removesuffix(".rgb565")
        if asset_key in keys:
            raise ValueError("duplicate screenshot asset key")
        keys.add(asset_key)
        if entries is not None:
            entries[asset_key] = (width, height, stride, raw, flag, compressed, start)
    if offset != len(data):
        raise ValueError("trailing screenshot index data")
    return keys


def pack_family_inventory(entries, parents):
    families = {}
    for key, payload in entries.items():
        families.setdefault(family_key(key, parents), {})[key] = payload
    aliases = {}
    redundant = []
    for family, members in sorted(families.items()):
        canonical = family if family in members else min(members)
        aliases.update({key: canonical for key in sorted(members) if key != canonical})
        images = len(set(members.values()))
        if images > 1:
            redundant.append(
                {"family": family, "keys": sorted(members), "extra_images": images - 1}
            )
    return {
        "image_families": len(families),
        "family_aliases": aliases,
        "redundant_family_images": sum(row["extra_images"] for row in redundant),
        "redundant_families": redundant,
    }


def attribute(text, name):
    match = re.search(rf"\b{re.escape(name)}\s*=\s*(['\"])(.*?)\1", text, re.S)
    return html.unescape(match[2]) if match else ""


def parent_map(data, excluded=None):
    if data.startswith(b"PK"):
        with zipfile.ZipFile(io.BytesIO(data)) as archive:
            files = [
                info
                for info in archive.infolist()
                if info.filename.lower().endswith(".xml")
            ]
            if len(files) != 1 or files[0].file_size > 512 * 1024 * 1024:
                raise ValueError("MAME ZIP must contain one bounded listxml document")
            data = archive.read(files[0])
    parents = {}
    name = ""
    # Decode only machine attributes, not the hundreds of MiB of ROM metadata.
    for match in re.finditer(rb"<machine\s+([^>]+)>|<softwarelist\b", data):
        if match[1] is None:
            if excluded is not None and name:
                excluded.add(name)
            continue
        attrs = match[1].decode("utf-8")
        name = attribute(attrs, "name")
        if name:
            parents[name] = attribute(attrs, "cloneof")
            if excluded is not None and (
                attribute(attrs, "isbios") == "yes"
                or attribute(attrs, "isdevice") == "yes"
                or attribute(attrs, "runnable") == "no"
            ):
                excluded.add(name)
    if not parents:
        raise ValueError("MAME listxml has no machines")
    return parents


def hbmame_parents(arguments, downloads, root):
    if arguments.hbmame_database:
        database_path = arguments.hbmame_database.resolve()
        provenance = {
            "source": str(database_path),
            "sha256": digest(database_path.read_bytes()),
        }
    else:
        owner = subprocess.run(
            ["gh", "repo", "view", "--json", "nameWithOwner", "--jq", ".nameWithOwner"],
            cwd=root,
            capture_output=True,
            text=True,
            check=True,
            timeout=30,
        ).stdout.strip()
        releases = downloads.api(f"repos/{owner}/releases?per_page=100")
        releases = [
            release
            for release in releases
            if not release["draft"]
            and re.fullmatch(r"game-databases-v[1-9][0-9]*", release["tag_name"])
        ]
        if not releases:
            raise ValueError(
                "no published game-database release for HBMAME parent metadata"
            )
        release = max(
            releases,
            key=lambda item: int(item["tag_name"].removeprefix("game-databases-v")),
        )
        version = release["tag_name"].removeprefix("game-databases-v")
        name = "game-databases-source-v" + version
        artifacts = downloads.api(
            f"repos/{owner}/actions/artifacts?name={name}&per_page=100"
        )["artifacts"]
        artifacts = [
            artifact
            for artifact in artifacts
            if artifact["name"] == name
            and not artifact["expired"]
            and artifact["workflow_run"]["head_branch"] == "main"
        ]
        if not artifacts:
            raise ValueError(
                "published HBMAME source artifact unavailable; supply --hbmame-database"
            )
        artifact = max(artifacts, key=lambda item: item["created_at"])
        expected = artifact.get("digest", "").removeprefix("sha256:")
        if (
            not re.fullmatch(r"[a-f0-9]{64}", expected)
            or artifact["size_in_bytes"] > 128 * 1024 * 1024
        ):
            raise ValueError("HBMAME source artifact has no valid bounded checksum")
        artifact_path = downloads.cache / ("artifact-" + expected)
        if artifact_path.exists():
            data = artifact_path.read_bytes()
        else:
            result = subprocess.run(
                ["gh", "api", f"repos/{owner}/actions/artifacts/{artifact['id']}/zip"],
                capture_output=True,
                check=True,
                timeout=180,
            )
            data = result.stdout
        if digest(data) != expected or len(data) != artifact["size_in_bytes"]:
            raise ValueError("HBMAME source artifact checksum/size mismatch")
        if not artifact_path.exists():
            artifact_path.write_bytes(data)
        with zipfile.ZipFile(io.BytesIO(data)) as archive:
            member = f"mister-magik-game-databases-source-v{version}.zip"
            if archive.getinfo(member).file_size > 128 * 1024 * 1024:
                raise ValueError("HBMAME source ZIP exceeds limit")
            with zipfile.ZipFile(io.BytesIO(archive.read(member))) as source:
                if source.getinfo("hbmame.sqlite3").file_size > 256 * 1024 * 1024:
                    raise ValueError("HBMAME database exceeds limit")
                database = source.read("hbmame.sqlite3")
        database_path = downloads.cache / ("hbmame-" + digest(database) + ".sqlite3")
        if not database_path.exists() or digest(database_path.read_bytes()) != digest(
            database
        ):
            database_path.write_bytes(database)
        provenance = {
            "source": artifact["archive_download_url"],
            "release": release["tag_name"],
            "artifact_sha256": expected,
            "sha256": digest(database),
            "workflow_revision": artifact["workflow_run"]["head_sha"],
        }
    with closing(
        sqlite3.connect(database_path.as_uri() + "?mode=ro", uri=True)
    ) as connection:
        parents = {
            key: parent or ""
            for key, parent in connection.execute(
                "SELECT setname, parent_setname FROM mame_machines"
            )
        }
    if not parents:
        raise ValueError("HBMAME database contains no parent metadata")
    return parents, {**provenance, "machines": len(parents)}


def inspect_mra(data):
    uncommented = re.sub(rb"<!--.*?-->", b"", data, flags=re.S)
    archives = set()
    for match in re.finditer(rb"<rom\b([^>]*)>", uncommented, re.S | re.I):
        for archive in attribute(match[1].decode("utf-8"), "zip").split("|"):
            key = normalize_setname(PurePosixPath(archive.strip()).name)
            if key:
                archives.add(key)
    # Extract header text only. Do not resolve external XML entities or execute anything.
    # Large inline ROM patches are irrelevant to identity. Skip comments while
    # locating the first real ROM element, then decode only the header.
    for token in re.finditer(rb"<!--.*?-->|<rom\b", data, re.S | re.I):
        if not token[0].startswith(b"<!--"):
            data = data[: token.start()]
            break
    text = re.sub(r"<!--.*?-->", "", data.decode("utf-8"), flags=re.S)
    if not re.search(r"<misterromdescription\b", text, re.I):
        raise ValueError("MRA has no misterromdescription root")

    def field(name):
        match = re.search(rf"<{name}\b[^>]*>(.*?)</{name}\s*>", text, re.S | re.I)
        return (
            html.unescape(
                re.sub(r"<!\[CDATA\[(.*?)\]\]>", r"\1", match[1], flags=re.S)
            ).strip()
            if match
            else ""
        )

    return {
        "title": field("name"),
        "setname": normalize_setname(field("setname")),
        "parent": normalize_setname(field("parent")),
        "rom_setnames": sorted(archives),
    }


def normalize_setname(value):
    value = value.strip().lower().removesuffix(".zip")
    return value if re.fullmatch(r"[a-z0-9][a-z0-9_.-]*", value) else ""


def reviewed_families(root):
    path = root / "crates/catalog/data/arcade-screenshot-families.tsv"
    data = path.read_bytes()
    links = {}
    for row in csv.reader(io.StringIO(data.decode()), delimiter="\t"):
        if not row or row[0].startswith("#"):
            continue
        if len(row) != 3 or not all(
            normalize_setname(key) == key and key for key in row[:2]
        ):
            raise ValueError("invalid reviewed screenshot family")
        key, family, evidence = row
        if key in links or key == family or urlparse(evidence).scheme != "https":
            raise ValueError(
                "duplicate, self-referencing or unverified screenshot family"
            )
        links[key] = family
    return links, {"source": str(path.relative_to(root)), "sha256": digest(data)}


def mra_family(mra, parents, reviewed):
    key = mra["setname"]
    if not key:
        return "", "missing_setname", []
    if key in reviewed:
        return family_key(reviewed[key], parents), "reviewed", []
    if key in parents:
        return family_key(key, parents), "machine_metadata", []
    parent = mra.get("parent", "")
    if parent != key and parent in parents:
        return family_key(parent, parents), "mra_parent", []
    families = sorted(
        {
            family_key(name, parents)
            for name in mra.get("rom_setnames", [])
            if name in parents
        }
    )
    if len(families) == 1:
        return families[0], "rom_family", []
    # Embedded games have an explicit identity and no external ROM family.
    if not mra.get("rom_setnames"):
        return key, "embedded_identity", []
    return "", "family_unresolved", families


def family_key(key, parents):
    seen = set()
    while parents.get(key):
        if key in seen:
            raise ValueError("cycle in MAME parent relationships")
        seen.add(key)
        key = parents[key]
    return key


def game_rows(mras, parents, keys, reviewed=None, non_games=None):
    non_games = non_games or {}
    reviewed = reviewed or {}
    parents = {**parents, **reviewed}
    available = {}
    for key in sorted(keys):
        available.setdefault(family_key(key, parents), key)
    groups = {}
    for mra in mras:
        key = mra["setname"]
        identity, resolution, candidates = mra_family(mra, parents, reviewed)
        is_game = key not in non_games
        if not is_game:
            identity, resolution = non_games[key], "system_metadata"
        matched = identity if identity in keys else available.get(identity, "")
        if not is_game:
            group_id, status = "non-game:" + identity, "not_game"
        elif identity:
            group_id, status = identity, "covered" if matched else "missing"
        elif key:
            group_id, status = "family-unresolved:" + key, "family_unresolved"
        else:
            group_id, status = "unresolved:" + mra["hash"], "identity_unresolved"
        game = groups.setdefault(
            group_id,
            {
                "identity": group_id,
                "screenshot_key": identity,
                "status": status,
                "matched_key": matched,
                "titles": set(),
                "setnames": set(),
                "channels": set(),
                "sources": set(),
                "mras": [],
            },
        )
        if matched and is_game:
            game.update(status="covered", matched_key=matched)
        game["titles"].add(mra["title"])
        game["setnames"].add(key)
        game["channels"].add(mra["channel"])
        game["sources"].add(mra["source"])
        game["mras"].append(
            {
                "setname": key,
                "family": identity,
                "resolution": resolution,
                "family_candidates": candidates,
                **{
                    name: mra[name]
                    for name in (
                        "source",
                        "path",
                        "url",
                        "channel",
                        "hash",
                        "hash_kind",
                    )
                },
            }
        )
    for game in groups.values():
        for field in ("titles", "setnames", "channels", "sources"):
            game[field] = sorted(game[field])
        game["mras"].sort(key=lambda row: (row["source"], row["path"]))
    return sorted(
        groups.values(), key=lambda game: (game["titles"][0].lower(), game["identity"])
    )


def error_text(error):
    detail = error.stderr if isinstance(error, subprocess.CalledProcessError) else error
    if isinstance(detail, bytes):
        detail = detail.decode(errors="replace")
    return str(detail or error).strip()


def table_value(value):
    return "; ".join(value) if isinstance(value, list) else str(value)


def markdown_table(headers, rows):
    def row(values):
        cells = [
            table_value(value).replace("|", "\\|").replace("\n", " ").replace("\r", " ")
            for value in values
        ]
        return "| " + " | ".join(cells) + " |"

    return [
        row(headers),
        row(["---"] * len(headers)),
        *(row(values) for values in rows),
    ]


def write_reports(output, report, baseline):
    games = report["games"]
    before = (
        {row["identity"]: row["status"] for row in baseline["games"]}
        if baseline
        else {}
    )
    for game in games:
        game["new_game"] = (
            bool(baseline)
            and game["status"] != "not_game"
            and game["identity"] not in before
        )
        game["new_gap"] = (
            bool(baseline)
            and game["status"] not in ("covered", "not_game")
            and before.get(game["identity"], "covered") == "covered"
        )
    report["summary"] = dict(Counter(game["status"] for game in games))
    report["summary"].update(
        games=len(games),
        mras=sum(len(game["mras"]) for game in games),
        beta_games=sum("beta" in game["channels"] for game in games),
        alpha_games=sum("alpha" in game["channels"] for game in games),
        new_games=sum(game["new_game"] for game in games),
        new_gaps=sum(game["new_gap"] for game in games),
    )
    report["baseline"] = baseline.get("generated_at") if baseline else None
    output.mkdir(parents=True, exist_ok=True)
    payload = json.dumps(report, indent=2) + "\n"
    (output / "audit.json").write_text(payload)
    columns = (
        "status",
        "screenshot_key",
        "titles",
        "channels",
        "sources",
        "setnames",
        "new_game",
        "new_gap",
        "mra_urls",
    )
    rows = [
        {
            **{field: table_value(game[field]) for field in columns[:-1]},
            "mra_urls": "; ".join(row["url"] for row in game["mras"]),
        }
        for game in games
    ]
    for filename, selected in (
        ("all-games.tsv", rows),
        (
            "missing.tsv",
            [row for row in rows if row["status"] not in ("covered", "not_game")],
        ),
    ):
        with (output / filename).open("w", newline="") as stream:
            writer = csv.DictWriter(stream, columns, delimiter="\t")
            writer.writeheader()
            writer.writerows(selected)
    lines = [
        "# Upstream Arcade screenshot audit",
        "",
        f"Generated: {report['generated_at']}",
        "",
        f"Published pack: {report['pack']['version']} ({report['pack']['entries']} screenshot keys).",
        "",
        f"Coverage: **{'complete' if report['complete'] else 'PARTIAL — see source errors below'}**.",
        "",
        (
            f"Pack storage: {report['pack'].get('image_families', report['pack']['entries'])} image families, "
            f"{len(report['pack'].get('family_aliases', {}))} compatibility aliases, "
            f"{report['pack'].get('redundant_family_images', 0)} redundant family images."
        ),
        "",
        "Counts represent screenshot families, using machine metadata, reviewed links, declared MRA parents and unambiguous ROM families. A group can appear in several channels.",
        "",
    ]
    lines += markdown_table(
        ["Status", "Games"],
        (
            (status, report["summary"].get(status, 0))
            for status in (
                "covered",
                "missing",
                "family_unresolved",
                "identity_unresolved",
                "not_game",
            )
        ),
    )
    link_columns = (
        "setname",
        "family",
        "matched_key",
        "status",
        "resolution",
        "family_candidates",
        "source",
        "path",
        "url",
    )
    with (output / "family-links.tsv").open("w", newline="") as stream:
        writer = csv.DictWriter(stream, link_columns, delimiter="\t")
        writer.writeheader()
        for game in games:
            for mra in game["mras"]:
                writer.writerow(
                    {
                        **{
                            field: table_value(mra.get(field, ""))
                            for field in link_columns
                        },
                        "matched_key": game["matched_key"],
                        "status": game["status"],
                    }
                )
    sections = (
        (
            "Missing beta and alpha screenshots",
            [
                game
                for game in games
                if game["status"] == "missing"
                and any(channel in game["channels"] for channel in ("beta", "alpha"))
            ],
        ),
        (
            "All missing screenshots",
            [game for game in games if game["status"] == "missing"],
        ),
        (
            "Unresolved family links",
            [game for game in games if game["status"] == "family_unresolved"],
        ),
        (
            "Systems and BIOS entries (not game screenshot requests)",
            [game for game in games if game["status"] == "not_game"],
        ),
        (
            "Unresolved screenshot identities",
            [game for game in games if game["status"] == "identity_unresolved"],
        ),
    )
    for title, selected in sections:
        lines += ["", "## " + title, ""] + markdown_table(
            ["Game names", "Screenshot key", "Channels", "Sources"],
            (
                [
                    game[field]
                    for field in ("titles", "screenshot_key", "channels", "sources")
                ]
                for game in selected
            ),
        )
    lines += ["", "## Source coverage", ""] + markdown_table(
        ["Feed", "MRAs", "Error"],
        (
            (source["title"], source.get("mras", 0), source.get("error", ""))
            for source in report["sources"]
        ),
    )
    if report["errors"]:
        lines += ["", "## Metadata errors", ""] + [
            "- " + error.replace("\n", " ") for error in report["errors"]
        ]
    (output / "report.md").write_text("\n".join(lines) + "\n")
    if report["complete"]:
        (output / "snapshot.json").write_text(payload)


def collect_sources(sources, downloads, workers):
    def read_source(source):
        data = downloads.fetch(source["url"])
        database = decode_database(data)
        rows, tree_error = source_rows(source, database, downloads)
        return rows, {
            **source,
            "sha256": digest(data),
            "timestamp": database.get("timestamp"),
            "mras": len(rows),
            "default_filter": database.get("default_options", {}).get("filter"),
            **({"error": "supplemental tree: " + tree_error} if tree_error else {}),
        }

    records, reports = [], []
    with ThreadPoolExecutor(max_workers=workers) as executor:
        tasks = {executor.submit(read_source, source): source for source in sources}
        for task in as_completed(tasks):
            source = tasks[task]
            try:
                rows, report = task.result()
                records.extend(rows)
                reports.append(report)
                print(
                    f"{source['id']}: {len(rows)} MRAs"
                    + (" (tree incomplete)" if report.get("error") else ""),
                    flush=True,
                )
            except (
                OSError,
                ValueError,
                KeyError,
                zipfile.BadZipFile,
                subprocess.SubprocessError,
            ) as error:
                reports.append({**source, "mras": 0, "error": error_text(error)})
                print(f"{source['id']}: source unavailable", flush=True)
    return records, reports


def inspect_records(records, downloads, workers):
    # Inspect identical payloads once, retaining every source/path/channel.
    by_hash = {}
    for row in records:
        by_hash.setdefault((row["hash_kind"], row["hash"]), []).append(row)

    def read_mra(rows):
        # Another mirror may publish the same verified payload.
        failure = None
        for row in rows:
            try:
                return inspect_mra(downloads.mra(row))
            except (OSError, ValueError, subprocess.SubprocessError) as error:
                failure = error
        raise ValueError(error_text(failure))

    inspected, errors = [], []
    with ThreadPoolExecutor(max_workers=workers) as executor:
        tasks = {executor.submit(read_mra, rows): rows for rows in by_hash.values()}
        for number, task in enumerate(as_completed(tasks), 1):
            rows = tasks[task]
            try:
                metadata = task.result()
                inspected.extend(
                    {
                        **row,
                        **metadata,
                        "title": metadata["title"] or PurePosixPath(row["path"]).stem,
                    }
                    for row in rows
                )
            except (OSError, ValueError, subprocess.SubprocessError) as error:
                errors.append(
                    f"{rows[0]['source']}: {rows[0]['path']}: {error_text(error)}"
                )
            if number % 100 == 0 or number == len(tasks):
                print(
                    f"Inspected {number}/{len(tasks)} distinct MRA payloads", flush=True
                )
    return inspected, errors


def run(arguments, root):
    output = arguments.output.resolve()
    downloads = Downloads(output / "cache")
    baseline_path = arguments.baseline or output / "snapshot.json"
    baseline = json.loads(baseline_path.read_text()) if baseline_path.exists() else None
    if arguments.baseline and not baseline_path.exists():
        raise ValueError(f"baseline does not exist: {baseline_path}")
    if baseline and (baseline.get("format") != FORMAT or not baseline.get("complete")):
        raise ValueError("baseline must be a complete upstream screenshot audit")
    registry = downloads.fetch(arguments.registry_url, limit=4 * 1024 * 1024)
    sources = registry_sources(registry)
    for number, url in enumerate(arguments.database_url):
        if not any(source["url"] == url for source in sources):
            sources.append(
                {
                    "id": f"extra-{number + 1}",
                    "db_id": "extra",
                    "title": url,
                    "url": url,
                }
            )
    manifest_data = downloads.fetch(arguments.manifest_url, limit=4 * 1024 * 1024)
    manifest = json.loads(manifest_data)
    packs = [
        pack
        for pack in manifest["packs"]
        if (pack["id"], pack["artifact"], pack["size"])
        == ("arcade", "screenshots", "320x320")
    ]
    if manifest.get("schema") != 1 or len(packs) != 1 or "index" not in packs[0]:
        raise ValueError("manifest must describe one indexed Arcade 320x320 pack")
    pack = packs[0]
    assets_root = urljoin(arguments.manifest_url, "/")
    entries = {}
    keys = pack_keys(
        downloads.fetch(
            urljoin(assets_root, pack["index"]["object"]),
            immutable=True,
            limit=8 * 1024 * 1024,
        ),
        pack,
        entries,
    )
    if arguments.mame_listxml:
        mame_data = arguments.mame_listxml.read_bytes()
        mame_source = str(arguments.mame_listxml.resolve())
    else:
        release = downloads.api("repos/mamedev/mame/releases/latest")
        assets = [
            asset for asset in release["assets"] if asset["name"].endswith("lx.zip")
        ]
        if len(assets) != 1:
            raise ValueError("latest MAME release has no unique listxml asset")
        mame_source = assets[0]["browser_download_url"]
        mame_data = downloads.fetch(mame_source, immutable=True)
    excluded = set()
    mame_parents = parent_map(mame_data, excluded)
    hb_parents, hb_provenance = hbmame_parents(arguments, downloads, root)
    all_parents = {**hb_parents, **mame_parents}
    non_games = {key: family_key(key, all_parents) for key in excluded}
    parents = {
        key: parent for key, parent in all_parents.items() if key not in excluded
    }
    reviewed, family_provenance = reviewed_families(root)
    if any(family not in parents for family in reviewed.values()):
        raise ValueError("reviewed screenshot family absent from machine metadata")
    print(
        f"Discovered {len(sources)} core/distribution feeds; pack {pack['version']}: {len(keys)} keys.",
        flush=True,
    )
    records, source_reports = collect_sources(sources, downloads, arguments.workers)
    inspected, errors = inspect_records(records, downloads, arguments.workers)
    report = {
        "format": FORMAT,
        "generated_at": datetime.now(timezone.utc).isoformat(),
        "complete": not errors
        and not any(source.get("error") for source in source_reports),
        "registry": {"url": arguments.registry_url, "sha256": digest(registry)},
        "mame": {
            "source": mame_source,
            "sha256": digest(mame_data),
            "machines": len(mame_parents),
        },
        "pack": {
            **pack,
            "entries": len(keys),
            **pack_family_inventory(entries, {**all_parents, **reviewed}),
            "manifest_url": arguments.manifest_url,
            "manifest_sha256": digest(manifest_data),
        },
        "hbmame": hb_provenance,
        "reviewed_families": family_provenance,
        "sources": sorted(source_reports, key=lambda source: source["id"]),
        "errors": sorted(errors),
        "games": game_rows(inspected, parents, keys, reviewed, non_games),
    }
    write_reports(output, report, baseline)
    print(json.dumps(report["summary"], indent=2))
    print(f"Report: {output / 'report.md'}")
    print(f"Missing games: {output / 'missing.tsv'}")
    return 0 if report["complete"] else 1
