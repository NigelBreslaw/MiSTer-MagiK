from contextlib import closing
from concurrent.futures import ThreadPoolExecutor
import hashlib
import io
import json
import struct
import sqlite3
import subprocess
from threading import Event
from types import SimpleNamespace
import zipfile

import pytest

from magik import cli, upstream_screenshots as audit


REGISTRY = b"""
DB_ID = "main"
class AllDBs:
    def __init__(self):
        # MAIN DISTRIBUTION
        self.MAIN = Database(db_id=DB_ID, db_url="https://example.test/main.zip", title="Main")
        # UNOFFICIAL CORES
        self.NEW_PROVIDER = Database(db_id="future", db_url="https://example.test/future.zip", title="New Provider")
        # ARTWORK
        self.ART = Database(db_id="art", db_url="https://example.test/art.zip", title="Arcade Screenshots")
raise RuntimeError("upstream code must never execute")
"""


def index_fixture(keys=("parent",)):
    archive_bytes, archive_sha = 1000, "a" * 64
    data = (
        b"MMIDX02\0"
        + struct.pack("<Q", archive_bytes)
        + archive_sha.encode()
        + struct.pack("<I", len(keys))
    )
    for key in keys:
        name = (key + ".rgb565").encode()
        data += struct.pack("<HIIIIBIQ", len(name), 1, 1, 2, 2, 1, 2, 0) + name
    pack = {
        "id": "arcade",
        "artifact": "screenshots",
        "size": "320x320",
        "version": "fixture",
        "bytes": archive_bytes,
        "sha256": archive_sha,
        "index": {
            "bytes": len(data),
            "sha256": audit.digest(data),
            "archive_bytes": archive_bytes,
            "archive_sha256": archive_sha,
            "object": "pack.idx",
        },
    }
    return data, pack


def payload(title, key):
    return f"<misterromdescription><name>{title}</name><setname>{key}</setname></misterromdescription>".encode()


def record(title, key, channel="public", source="main", path="game.mra"):
    data = payload(title, key)
    return {
        "title": title,
        "setname": key,
        "source": source,
        "channel": channel,
        "path": path,
        "url": "https://example.test/" + path,
        "hash": hashlib.md5(data, usedforsecurity=False).hexdigest(),
        "hash_kind": "md5",
        "bytes": len(data),
    }


def test_discovery_includes_new_core_providers_without_executing_registry():
    sources = audit.registry_sources(REGISTRY)
    assert {row["db_id"] for row in sources} == {"main", "future"}
    assert "art" not in {row["db_id"] for row in sources}


def test_beta_alpha_tags_and_folder_inheritance_are_decoded():
    database = {
        "tag_dictionary": {
            "jtbeta": 12,
            "coinopcollectionalpha": 13,
            "arcadealpha68k": 14,
        },
        "folders": {"_Arcade/beta": {"tags": [12]}},
    }
    assert audit.channel(database, {"tags": []}, "_Arcade/beta/game.mra") == "beta"
    assert audit.channel(database, {"tags": [12, 13]}, "game.mra") == "alpha"
    assert audit.channel(database, {"tags": [14]}, "game.mra") == "public"


def test_verified_index_rejects_wrong_archive_truncation_and_duplicate_keys():
    data, pack = index_fixture()
    assert audit.pack_keys(data, pack) == {"parent"}
    bad_pack = {**pack, "sha256": "b" * 64}
    with pytest.raises(ValueError, match="archive identity"):
        audit.pack_keys(data, bad_pack)
    with pytest.raises(ValueError, match="checksum/size"):
        audit.pack_keys(data[:-1], pack)
    data, pack = index_fixture(("parent", "parent"))
    with pytest.raises(ValueError, match="duplicate"):
        audit.pack_keys(data, pack)


def test_parent_coverage_deduplicates_clones_but_preserves_sources_and_channels():
    rows = [
        record("Clone", "clone", "beta"),
        record("Parent", "parent", "public", "other"),
    ]
    games = audit.game_rows(rows, {"clone": "parent", "parent": ""}, {"parent"})
    assert len(games) == 1
    assert games[0]["status"] == "covered"
    assert games[0]["matched_key"] == "parent"
    assert games[0]["channels"] == ["beta", "public"]
    assert games[0]["sources"] == ["main", "other"]


def test_equal_titles_do_not_merge_distinct_games_and_blank_keys_remain_unresolved():
    games = audit.game_rows(
        [record("Same", "one"), record("Same", "two"), record("Blank", "")], {}, set()
    )
    assert len(games) == 3
    assert [game["status"] for game in games].count("missing") == 2
    assert [game["status"] for game in games].count("identity_unresolved") == 1


def test_xml_comments_entities_and_blank_setname_are_not_guessed():
    data = b"<misterromdescription><!--<setname>fake</setname>--><name><![CDATA[A &amp; B]]></name><setname> </setname><rom><setname>fake2</setname></rom></misterromdescription>"
    assert audit.inspect_mra(data) == ("A & B", "")


def test_source_tree_recovers_omitted_alternatives_and_preserves_unclassified_channel():
    revision = "a" * 40
    base = f"https://raw.githubusercontent.com/example/repo/{revision}/"
    source = {"id": "main"}
    database = {"base_files_url": base, "files": {}}
    downloads = SimpleNamespace(
        api=lambda _: {
            "tree": [
                {
                    "type": "blob",
                    "path": "_Arcade/_alternatives/new.mra",
                    "sha": "b" * 40,
                    "size": 10,
                }
            ]
        }
    )
    rows, error = audit.source_rows(source, database, downloads)
    assert not error
    assert len(rows) == 1
    assert rows[0]["channel"] == "unknown"
    assert rows[0]["hash_kind"] == "git"


def test_truncated_source_tree_is_a_coverage_failure():
    database = {
        "base_files_url": "https://raw.githubusercontent.com/example/repo/"
        + "a" * 40
        + "/",
        "files": {},
    }
    rows, error = audit.source_rows(
        {"id": "main"}, database, SimpleNamespace(api=lambda _: {"truncated": True})
    )
    assert rows == []
    assert "truncated" in error


def test_cached_mra_bytes_are_still_verified(tmp_path):
    downloads = audit.Downloads(tmp_path)
    row = record("Title", "key")
    (tmp_path / ("mra-" + row["hash"])).write_bytes(b"corrupt cached bytes")
    with pytest.raises(ValueError, match="checksum/size"):
        downloads.mra(row)


@pytest.mark.parametrize("incomplete", [False, True])
def test_end_to_end_reports_channels_missing_keys_failures_and_baseline(
    monkeypatch, tmp_path, incomplete
):
    index, pack = index_fixture()
    mras = {
        "parent.mra": payload("Clone", "clone"),
        "beta.mra": payload("Future beta", "futurebeta"),
        "alpha.mra": payload("Future alpha", "futurealpha"),
        "blank.mra": payload("Unknown", ""),
    }
    entries = {
        path: {
            "hash": hashlib.md5(data, usedforsecurity=False).hexdigest(),
            "size": len(data),
            "tags": [12] if path == "beta.mra" else [13] if path == "alpha.mra" else [],
        }
        for path, data in mras.items()
    }
    database = {
        "base_files_url": "https://example.test/mras/",
        "files": entries,
        "tag_dictionary": {"jtbeta": 12, "coinopcollectionalpha": 13},
    }

    def fetch(self, url, **_):
        if url == audit.REGISTRY_URL:
            return REGISTRY
        if url == audit.MANIFEST_URL:
            return json.dumps({"schema": 1, "packs": [pack]}).encode()
        if url.endswith("pack.idx"):
            return index
        if incomplete and url == "https://example.test/future.zip":
            raise ValueError("feed offline")
        return json.dumps(database).encode()

    monkeypatch.setattr(audit.Downloads, "fetch", fetch)
    monkeypatch.setattr(audit.Downloads, "mra", lambda self, row: mras[row["path"]])
    xml = tmp_path / "mame.xml"
    xml.write_bytes(
        b'<mame><machine name="clone" cloneof="parent"></machine><machine name="parent"></machine></mame>'
    )
    baseline = {
        "format": audit.FORMAT,
        "complete": True,
        "generated_at": "before",
        "games": [
            {"identity": "parent", "status": "covered"},
            {"identity": "futurebeta", "status": "covered"},
        ],
    }
    (tmp_path / "snapshot.json").write_text(json.dumps(baseline))
    hb = tmp_path / "hbmame.sqlite3"
    with closing(sqlite3.connect(hb)) as connection, connection:
        connection.execute(
            "CREATE TABLE mame_machines (setname TEXT, parent_setname TEXT)"
        )
        connection.execute("INSERT INTO mame_machines VALUES ('hbhack', 'parent')")
    args = SimpleNamespace(
        hbmame_database=hb,
        output=tmp_path,
        baseline=None,
        database_url=[],
        registry_url=audit.REGISTRY_URL,
        manifest_url=audit.MANIFEST_URL,
        mame_listxml=xml,
        workers=2,
    )
    assert audit.run(args, tmp_path) == int(incomplete)
    report = json.loads((tmp_path / "audit.json").read_text())
    assert report["summary"]["covered"] == 1
    assert report["summary"]["missing"] == 2
    assert report["summary"]["identity_unresolved"] == 1
    assert report["summary"]["new_gaps"] == 3
    assert report["summary"]["beta_games"] == 1
    assert report["summary"]["alpha_games"] == 1
    assert report["complete"] == (not incomplete)
    assert "Future beta" in (tmp_path / "missing.tsv").read_text()
    assert "Clone" not in (tmp_path / "missing.tsv").read_text()
    if incomplete:
        assert json.loads((tmp_path / "snapshot.json").read_text()) == baseline
        assert "PARTIAL" in (tmp_path / "report.md").read_text()


def test_host_command_never_dispatches_device_operations(monkeypatch, tmp_path):
    monkeypatch.setattr(
        "sys.argv", ["scripts/magik", "upstream-screenshots", "--output", str(tmp_path)]
    )
    monkeypatch.setattr(audit, "run", lambda args, root: 0)
    monkeypatch.setattr(
        cli, "dispatch", lambda *_: pytest.fail("device dispatch called")
    )
    assert cli.main() == 0


def test_zip_database_rejects_multiple_documents():
    stream = io.BytesIO()
    with zipfile.ZipFile(stream, "w") as archive:
        archive.writestr("one.json", "{}")
        archive.writestr("two.json", "{}")
    with pytest.raises(ValueError, match="one bounded"):
        audit.decode_database(stream.getvalue())


def test_hbmame_hacks_resolve_to_published_family(tmp_path):
    database = tmp_path / "hbmame.sqlite3"
    with closing(sqlite3.connect(database)) as connection, connection:
        connection.execute(
            "CREATE TABLE mame_machines (setname TEXT, parent_setname TEXT)"
        )
        connection.executemany(
            "INSERT INTO mame_machines VALUES (?,?)",
            [("hack", "clone"), ("clone", "parent")],
        )
    parents, provenance = audit.hbmame_parents(
        SimpleNamespace(hbmame_database=database), None, tmp_path
    )
    games = audit.game_rows([record("Hack", "hack", "unknown")], parents, {"parent"})
    assert games[0]["status"] == "covered"
    assert games[0]["matched_key"] == "parent"
    assert provenance["machines"] == 2


def test_host_metadata_failure_is_reported_without_device_dispatch(monkeypatch, capsys):
    monkeypatch.setattr("sys.argv", ["scripts/magik", "upstream-screenshots"])

    def fail(*_):
        raise sqlite3.DatabaseError("invalid parent metadata")

    monkeypatch.setattr(audit, "run", fail)
    monkeypatch.setattr(
        cli, "dispatch", lambda *_: pytest.fail("device dispatch called")
    )
    assert cli.main() == 2
    assert "invalid parent metadata" in capsys.readouterr().err


def test_clone_only_screenshot_does_not_hide_a_missing_family_image():
    rows = [record("Clone", "clone"), record("Parent", "parent")]
    games = audit.game_rows(rows, {"clone": "parent", "parent": ""}, {"clone"})
    assert len(games) == 1
    assert games[0]["screenshot_key"] == "parent"
    assert games[0]["status"] == "missing"


def test_mutable_downloads_do_not_create_duplicate_url_cache_files(
    monkeypatch, tmp_path
):
    data = b"upstream metadata"
    monkeypatch.setattr(
        audit.subprocess, "run", lambda *_args, **_kwargs: SimpleNamespace(stdout=data)
    )
    downloads = audit.Downloads(tmp_path)
    assert downloads.fetch("https://example.test/fresh") == data
    assert list(tmp_path.iterdir()) == []
    assert downloads.fetch("https://example.test/pinned", immutable=True) == data
    assert len(list(tmp_path.iterdir())) == 1


def test_non_object_feed_is_rejected_as_metadata_error():
    with pytest.raises(ValueError, match="files map"):
        audit.decode_database(b"[]")


@pytest.mark.parametrize("stderr", [b"feed offline", "feed offline"])
def test_command_error_preserves_byte_and_text_details(stderr):
    error = audit.subprocess.CalledProcessError(1, ["gh"], stderr=stderr)
    assert audit.error_text(error) == "feed offline"


def test_warm_mra_cache_is_verified_without_rewriting(monkeypatch, tmp_path):
    downloads = audit.Downloads(tmp_path)
    row = record("Title", "key")
    data = payload("Title", "key")
    cache = tmp_path / ("mra-" + row["hash"])
    cache.write_bytes(data)
    modified = cache.stat().st_mtime_ns
    monkeypatch.setattr(
        downloads, "fetch", lambda *args, **kwargs: pytest.fail("cache miss")
    )
    assert downloads.mra(row) == data
    assert cache.stat().st_mtime_ns == modified


def test_mra_header_ignores_comment_rom_tokens_and_inline_patch_bytes():
    data = (
        b"<misterromdescription><!--<rom>fake</rom>--><name>Actual</name><setname>actual</setname><rom>"
        + b"\xff" * 1000
    )
    assert audit.inspect_mra(data) == ("Actual", "actual")


def test_mame_parent_attributes_decode_entities_from_xml_and_zip():
    data = b'<mame><machine name="parent"/><machine name="clone" cloneof="par&#101;nt"><description>\xc3\xa9</description></machine></mame>'
    assert audit.parent_map(data) == {"parent": "", "clone": "parent"}
    zipped = io.BytesIO()
    with zipfile.ZipFile(zipped, "w") as archive:
        archive.writestr("mame.xml", data)
    assert audit.parent_map(zipped.getvalue()) == {"parent": "", "clone": "parent"}


@pytest.mark.parametrize("failed", [False, True])
def test_concurrent_api_requests_share_response_or_failure(
    monkeypatch, tmp_path, failed
):
    started, release = Event(), Event()
    calls = []

    def run(argv, **kwargs):
        calls.append(argv)
        started.set()
        assert release.wait(5)
        if failed:
            raise subprocess.CalledProcessError(1, argv, stderr=b"unavailable")
        return SimpleNamespace(stdout=b'{"tree": []}')

    monkeypatch.setattr(audit.subprocess, "run", run)
    downloads = audit.Downloads(tmp_path)
    with ThreadPoolExecutor(max_workers=2) as pool:
        first = pool.submit(downloads.api, "shared/tree")
        assert started.wait(5)
        second = pool.submit(downloads.api, "shared/tree")
        release.set()
        for result in (first, second):
            if failed:
                with pytest.raises(subprocess.CalledProcessError):
                    result.result(timeout=5)
            else:
                assert result.result(timeout=5) == {"tree": []}
    assert len(calls) == 1
    # No metadata survives into another audit invocation.
    fresh = audit.Downloads(tmp_path)
    if failed:
        with pytest.raises(subprocess.CalledProcessError):
            fresh.api("shared/tree")
    else:
        fresh.api("shared/tree")
    assert len(calls) == 2
