# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

"""CI-only delivery checks: real Downloader and the shipped ARM bootstrap."""

from __future__ import annotations

import copy
import hashlib
import json
import os
import subprocess
import sys
import tempfile
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from typing import Any
from urllib.parse import urlsplit

from . import distribution as dist
from .common import atomic_write, sha256_file

DOWNLOADER_REVISION = "5d0771359ae396aaea64453e6791ac87781d78f4"
# Keep the source and the settings exercised by this gate explicit.  In
# particular, PC_LAUNCHER is deliberately absent from the direct test below:
# Downloader changes a requested balanced/fastest run to exhaustive in that
# mode, hiding the cache lifecycle this gate is intended to qualify.
DOWNLOADER_MODES = ("balanced", "fastest", "exhaustive", "verify_integrity")
DELETION_POLICIES = (0, 1, 2)
CASES = (
    "fresh",
    "upgrade",
    "deletion-disabled",
    "cores-only-deletion",
    "cached-same-version",
    "legacy-uninstaller-negative-control",
)


def _fixture_database(db_id: str, files: dict[str, tuple[bytes, str]]) -> dict[str, Any]:
    """Create the small v1 database used by the lifecycle regression.

    These are intentionally generated test bytes, rather than release assets.
    The delivery gate below still runs the real pinned Downloader executable;
    this fixture isolates cache semantics and keeps the historical failure
    reproducible even when no release candidate is available locally.
    """
    return {
        "v": 1,
        "db_id": db_id,
        "timestamp": 1_700_000_000,
        "files": {
            path: {
                "hash": hashlib.md5(payload).hexdigest(),
                "size": len(payload),
                "url": url,
            }
            for path, (payload, url) in files.items()
        },
        "folders": {
            parent: {}
            for path in files
            for parent in (path.rsplit("/", 1)[0] + "/",)
            if "/" in path
        },
    }


def direct_cached_reinstall_regression(source: Path) -> None:
    """Exercise Downloader's retained store and prove the old uninstall bug.

    No PC launcher is provided.  The first and second unchanged runs retain
    all store/fingerprint/free-space files.  A legacy uninstaller then removes
    only the owned payload, leaving Downloader's fingerprint.  The next run
    skips the database and makes no payload request, which is the regression
    that the manager fix must turn into a successful reinstall.
    """
    payload = b"sanitized MagiK lifecycle fixture\n"
    other_payload = b"unrelated database fixture\n"
    with tempfile.TemporaryDirectory(prefix="magik-cached-regression-") as temporary:
        fat = Path(temporary)
        (fat / "Scripts").mkdir(parents=True)
        (fat / "MiSTer.ini").write_text("[MiSTer]\nmain=MiSTer\n")
        requests: list[str] = []
        content: dict[str, bytes] = {}
        current = _fixture_database(
            "mister_magik", {"mister-magik/fixture.txt": (payload, "/mister-magik/fixture.txt")}
        )
        unrelated = _fixture_database(
            "unrelated", {"unrelated/fixture.txt": (other_payload, "/unrelated/fixture.txt")}
        )

        class Handler(BaseHTTPRequestHandler):
            def do_GET(self):
                path = urlsplit(self.path).path
                requests.append(path)
                data = content.get(path)
                self.send_response(200 if data is not None else 404)
                self.send_header("Content-Length", str(len(data) if data is not None else 0))
                self.end_headers()
                if data is not None:
                    self.wfile.write(data)

            def log_message(self, format, *args):
                pass

        with ThreadingHTTPServer(("127.0.0.1", 0), Handler) as server:
            worker = threading.Thread(target=server.serve_forever, daemon=True)
            worker.start()
            try:
                base = f"http://lifecycle.example.test:{server.server_port}"
                current["files"]["mister-magik/fixture.txt"]["url"] = base + "/mister-magik/fixture.txt"
                unrelated["files"]["unrelated/fixture.txt"]["url"] = base + "/unrelated/fixture.txt"
                content["/mister-magik-db.json"] = json.dumps(current).encode()
                content["/unrelated-db.json"] = json.dumps(unrelated).encode()
                content["/mister-magik/fixture.txt"] = payload
                content["/unrelated/fixture.txt"] = other_payload
                environment = {
                    "PATH": os.environ["PATH"],
                    "LANG": os.environ.get("LANG", "en_US.UTF-8"),
                    "DOWNLOADER_INI_PATH": str(fat / "downloader.ini"),
                    "FORCED_BASE_PATH": str(fat),
                    "ALLOW_REBOOT": "0",
                    "UPDATE_LINUX": "false",
                    "FAIL_ON_FILE_ERROR": "true",
                    "DEFAULT_DB_ID": "mister_magik",
                    "DEFAULT_DB_URL": base + "/mister-magik-db.json",
                    "DOWNLOADER_OUTPUT": "dlp1-ltsv",
                    "DEBUG": "true",
                    "LOGLEVEL": "debug",
                    "PYTHONDONTWRITEBYTECODE": "1",
                    "HTTP_PROXY": f"http://127.0.0.1:{server.server_port}",
                    "HTTPS_PROXY": f"http://127.0.0.1:{server.server_port}",
                }

                def run(mode: str, db_id: str = "mister_magik") -> str:
                    (fat / "downloader.ini").write_text(
                        "[MiSTer]\nallow_delete = 1\nallow_reboot = 0\n"
                        f"update_linux = false\nfile_checking = {mode}\n"
                        f"[{db_id}]\n"
                        f"db_url = {base}/{'mister-magik' if db_id == 'mister_magik' else 'unrelated'}-db.json\n"
                    )
                    result = subprocess.run(
                        [sys.executable, str(source / "src"), "--run-only", db_id],
                        cwd=fat,
                        env=environment,
                        capture_output=True,
                        text=True,
                        timeout=180,
                        check=False,
                    )
                    if result.returncode:
                        raise ValueError(
                            f"direct Downloader regression failed ({mode}/{db_id}): "
                            f"{result.stdout[-6000:]} {result.stderr[-3000:]}"
                        )
                    return result.stdout

                for mode in DOWNLOADER_MODES:
                    requests.clear()
                    first = run(mode)
                    mode_values = {"fastest": 0, "balanced": 1, "exhaustive": 2, "verify_integrity": 3}
                    expected_mode = mode_values[mode]
                    changed = "File checking changed from" in first
                    if mode in ("exhaustive", "verify_integrity", "fastest") and changed:
                        raise ValueError(f"Downloader silently changed effective mode: {mode}")
                    if mode == "balanced" and changed and 'from "1" to "2"' not in first:
                        raise ValueError("balanced mode changed to an unexpected effective mode")
                    if not changed and expected_mode != 0 and mode == "balanced" and os.getenv("MISTER_REQUIRE_MEDIA_FAT_MOUNT") == "1":
                        raise ValueError("balanced mode did not resolve with a real /media/fat mount")
                    managed = fat / "mister-magik/fixture.txt"
                    if not managed.is_file():
                        raise ValueError("direct Downloader fixture was not installed")
                    requests.clear()
                    run(mode)
                    if any(path == "/mister-magik/fixture.txt" for path in requests):
                        raise ValueError(f"unchanged cached run fetched payload in {mode} mode")
                    if mode == "fastest":
                        # Simulate the old helper: it deleted owned files but
                        # did not unregister the database or alter its store
                        # state.  FASTEST must therefore skip the same feed.
                        managed.unlink()
                        state_root = fat / "Scripts/.config/downloader"
                        state_files = (
                            "downloader.json",
                            "downloader_fingerprints.json",
                            "previous_free_space.json",
                            "downloader.last_successful_run",
                        )
                        before_state = {
                            state_root / name: (state_root / name).read_bytes()
                            for name in state_files
                            if (state_root / name).is_file()
                        }
                        requests.clear()
                        run("fastest")
                        if any(path == "/mister-magik/fixture.txt" for path in requests):
                            raise ValueError("legacy negative control unexpectedly downloaded payload")
                        if managed.exists():
                            raise ValueError("negative control did not reproduce skipped reinstall")
                        after_state = {
                            state_root / name: (state_root / name).read_bytes()
                            for name in state_files
                            if (state_root / name).is_file()
                        }
                        if before_state != after_state:
                            changed_state = [
                                path.name
                                for path in set(before_state) | set(after_state)
                                if before_state.get(path) != after_state.get(path)
                            ]
                            raise ValueError(
                                "negative control changed retained Downloader state: "
                                + ",".join(sorted(changed_state))
                            )
                    if mode == "balanced":
                        # A real unrelated update is the only allowed source
                        # of a new free-space record in this regression.
                        run("exhaustive", "unrelated")
                # Balanced negative control: remove the payload, make a real
                # unrelated database update (which records free space), then
                # run the unchanged MagiK feed.  CI enables the mount assertion
                # and requires this to skip; local macOS lacks /proc/mounts.
                managed = fat / "mister-magik/fixture.txt"
                if managed.exists():
                    managed.unlink()
                run("exhaustive", "unrelated")
                requests.clear()
                run("balanced")
                if os.getenv("MISTER_REQUIRE_MEDIA_FAT_MOUNT") == "1":
                    if any(path == "/mister-magik/fixture.txt" for path in requests):
                        raise ValueError("balanced cached negative control fetched payload")
                    if managed.exists():
                        raise ValueError("balanced cached negative control unexpectedly restored payload")
            finally:
                server.shutdown()
                worker.join(timeout=5)


def smoke(root: Path) -> None:
    manager = root / dist.PUBLIC["manager"].removeprefix("/media/fat/")
    header = manager.read_bytes()[:20]
    if header[:6] != b"\x7fELF\x01\x01" or header[18:20] != b"\x28\x00":
        raise ValueError("delivery smoke requires the shipped ARM ELF manager")
    before = dist._inventory(root)
    result = subprocess.run(
        ["/bin/sh", str(root / dist.LAUNCHER), "verify-platform"],
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
    if result.returncode or "verified platform" not in result.stdout:
        raise ValueError(
            f"shipped installer verification failed: {result.stdout[-1500:]} {result.stderr[-1500:]}"
        )
    if before != dist._inventory(root):
        raise ValueError("installer verification changed package bytes")


def downloader_test(
    candidate: Path, *, channel: str, source: Path, run_smoke: bool = True
) -> None:
    source = source.resolve()
    revision = subprocess.check_output(
        ["git", "-C", str(source), "rev-parse", "HEAD"], text=True
    ).strip()
    dirty = subprocess.check_output(
        ["git", "-C", str(source), "status", "--porcelain", "--untracked-files=no"],
        text=True,
    ).strip()
    if revision != DOWNLOADER_REVISION or dirty:
        raise ValueError("Downloader source must match the clean pinned revision")
    direct_cached_reinstall_regression(source)
    original = dist.read_json(candidate / f"mister-magik-{channel}-db.json")
    receipt = dist.read_json(candidate / "release-assets.json")
    expected = {entry["path"]: entry["sha256"] for entry in receipt["files"]}
    content = {
        "/" + entry["asset"]: (candidate / entry["asset"]).read_bytes()
        for entry in receipt["files"]
    }
    legacy = b"# obsolete internal helper\nexit 99\n"
    content["/legacy-helper"] = legacy
    old_launcher = b'#!/bin/sh\n. "${0%/*}/MiSTer-MagiK.platform-v3.constants.sh"\n'
    content["/old-launcher"] = old_launcher

    class Handler(BaseHTTPRequestHandler):
        def do_GET(self):
            data = content.get(urlsplit(self.path).path)
            self.send_response(200 if data is not None else 404)
            self.send_header(
                "Content-Length", str(len(data) if data is not None else 0)
            )
            self.end_headers()
            if data is not None:
                self.wfile.write(data)

        def log_message(self, format, *args):
            pass

    with ThreadingHTTPServer(("127.0.0.1", 0), Handler) as server:
        worker = threading.Thread(target=server.serve_forever, daemon=True)
        worker.start()
        try:
            base = "http://release.example.test"
            proxy = f"http://127.0.0.1:{server.server_port}"
            # Only transport URLs change in this isolated copy. Installed bytes,
            # paths, sizes and hashes remain the actual generated payload.
            current = copy.deepcopy(original)
            for name, item in current["files"].items():
                item["url"] = base + "/" + dist.asset_name(name)
            previous = copy.deepcopy(current)
            previous["timestamp"] = int(current["timestamp"]) - 1
            previous["files"][dist.LEGACY_HELPER] = {
                "url": base + "/legacy-helper",
                "size": len(legacy),
                "hash": hashlib.md5(legacy).hexdigest(),
            }
            previous["files"][dist.LAUNCHER] = {
                "url": base + "/old-launcher",
                "size": len(old_launcher),
                "hash": hashlib.md5(old_launcher).hexdigest(),
            }
            for case in CASES:
                with tempfile.TemporaryDirectory(
                    prefix="magik-downloader-"
                ) as temporary:
                    fat = Path(temporary)
                    scripts = fat / "Scripts"
                    scripts.mkdir()
                    unrelated = scripts / "user-script.sh"
                    unrelated.write_bytes(b"user-owned\n")
                    (fat / "MiSTer.ini").write_bytes(b"[MiSTer]\nmain=MiSTer\n")
                    (fat / "test-inittab").write_bytes(b"user boot configuration\n")
                    protected = {
                        name: (fat / name).read_bytes()
                        for name in (
                            "Scripts/user-script.sh",
                            "MiSTer.ini",
                            "test-inittab",
                        )
                    }
                    deletion = {"deletion-disabled": 0, "cores-only-deletion": 2}.get(
                        case, 1
                    )
                    (fat / "downloader.ini").write_text(
                        f"[MiSTer]\nallow_delete = {deletion}\nallow_reboot = 0\nupdate_linux = false\nfile_checking = verify_integrity\n"
                        f"[mister_magik]\ndb_url = {base}/database.json\n"
                    )
                    environment = {
                        "PATH": os.environ["PATH"],
                        "LANG": os.environ.get("LANG", "en_US.UTF-8"),
                        "DOWNLOADER_INI_PATH": str(fat / "downloader.ini"),
                        "FORCED_BASE_PATH": str(fat),
                        "ALLOW_REBOOT": "0",
                        "UPDATE_LINUX": "false",
                        "FAIL_ON_FILE_ERROR": "true",
                        "DEFAULT_DB_ID": "mister_magik",
                        "DEFAULT_DB_URL": base + "/database.json",
                        "PYTHONDONTWRITEBYTECODE": "1",
                        "HTTP_PROXY": proxy,
                        "HTTPS_PROXY": proxy,
                    }

                    def download(database, mode="balanced", fat=fat, environment=environment, case=case):
                        content["/database.json"] = json.dumps(database).encode()
                        config = fat / "downloader.ini"
                        config.write_text(
                            f"[MiSTer]\nallow_delete = {deletion}\nallow_reboot = 0\n"
                            f"update_linux = false\nfile_checking = {mode}\n"
                            f"[mister_magik]\ndb_url = {base}/database.json\n"
                        )
                        result = subprocess.run(
                            [
                                sys.executable,
                                str(source / "src"),
                                "--run-only",
                                "mister_magik",
                            ],
                            cwd=fat,
                            env=environment,
                            capture_output=True,
                            text=True,
                            timeout=180,
                            check=False,
                        )
                        if result.returncode:
                            raise ValueError(
                                f"Downloader {case} failed: {result.stdout[-2000:]} {result.stderr[-1000:]}"
                            )
                        return result.stdout

                    # Every configured checking mode is exercised without the
                    # PC launcher.  The mode resolver is expected to leave
                    # exhaustive and verify_integrity unchanged; balanced and
                    # fastest must reach FASTEST only after their first run
                    # records a real store/free-space snapshot.
                    for mode in DOWNLOADER_MODES:
                        if case != "fresh":
                            download(previous, mode)
                            if (fat / dist.LEGACY_HELPER).read_bytes() != legacy:
                                raise ValueError("old managed layout was not installed")
                        output = download(current, mode)
                        for name, digest in expected.items():
                            if (
                                not (fat / name).is_file()
                                or sha256_file(fat / name) != digest
                            ):
                                raise ValueError(
                                    f"Downloader changed/missed payload: {name}; {output[-2000:]}"
                                )
                        if (fat / dist.LEGACY_HELPER).exists() != (deletion != 1):
                            raise ValueError(f"Downloader deletion policy mismatch: {case}")
                        if {
                            name: (fat / name).read_bytes() for name in protected
                        } != protected:
                            raise ValueError("Downloader changed user/boot files")
                        if run_smoke:
                            smoke(fat)
        finally:
            server.shutdown()
            worker.join(timeout=5)


def run(candidate: Path, *, channel: str, source: Path) -> dict[str, Any]:
    candidate = candidate.resolve()
    validated = dist.verify(candidate, channel=channel)
    receipt = dist.read_json(candidate / "release-assets.json")
    with tempfile.TemporaryDirectory(prefix="magik-shipped-installer-") as temporary:
        root = Path(temporary)
        dist.extract_package(candidate / receipt["archive"], root / "zip")
        dist.reconstruct(candidate, channel, receipt, root / "downloaded")
        smoke(root / "zip")
        smoke(root / "downloaded")
    downloader_test(candidate, channel=channel, source=source)
    evidence = {
        "format": "mister-magik-delivery-evidence-v1",
        "candidate_id": validated["candidate_id"],
        "downloader_revision": DOWNLOADER_REVISION,
        "installer": "shipped-arm-verify-platform",
        "cases": list(CASES),
        "validation": "passed",
    }
    atomic_write(candidate / dist.EVIDENCE, dist.canonical_json(evidence))
    dist.write_checksums(candidate)
    return evidence


def require_evidence(candidate: Path, validated: dict[str, Any]) -> None:
    expected = {
        "format": "mister-magik-delivery-evidence-v1",
        "candidate_id": validated["candidate_id"],
        "downloader_revision": DOWNLOADER_REVISION,
        "installer": "shipped-arm-verify-platform",
        "cases": list(CASES),
        "validation": "passed",
    }
    if dist.read_json(candidate / dist.EVIDENCE) != expected:
        raise ValueError(
            "passing delivery evidence for this exact candidate is required"
        )
