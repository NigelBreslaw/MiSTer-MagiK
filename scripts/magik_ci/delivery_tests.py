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
CASES = ("fresh", "upgrade", "deletion-disabled", "cores-only-deletion")


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
                        "PC_LAUNCHER": str(fat / "pc_launcher.py"),
                        "PC_LAUNCHER_NO_WAIT": "1",
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

                    def download(database, fat=fat, environment=environment, case=case):
                        content["/database.json"] = json.dumps(database).encode()
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

                    if case != "fresh":
                        download(previous)
                        if (fat / dist.LEGACY_HELPER).read_bytes() != legacy:
                            raise ValueError("old managed layout was not installed")
                    output = download(current)
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
