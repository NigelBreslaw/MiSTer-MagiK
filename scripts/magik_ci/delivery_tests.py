# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

"""CI-only delivery checks: real Downloader and the shipped ARM bootstrap."""

from __future__ import annotations

import copy
import hashlib
import json
import os
import shutil
import signal
import subprocess
import sys
import tempfile
import threading
import zipfile
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from typing import Any, TypedDict
from urllib.parse import urlsplit

from . import distribution as dist
from .common import atomic_write, sha256_file


class _FailureResponse(TypedDict):
    status: int
    db: bytes
    payload: bytes
    delay: float


class _DeliveryServer(ThreadingHTTPServer):
    content: dict[str, bytes]
    requests: list[str]
    response_delays: dict[str, float]
    response_statuses: dict[str, int]


_PIN_RECORD = Path(__file__).with_name("dependency_pins.json")
DEPENDENCY_PINS = json.loads(_PIN_RECORD.read_text())
DOWNLOADER_REVISION = DEPENDENCY_PINS["downloader_source"]
DEVICE_DOWNLOADER_REVISION = DEPENDENCY_PINS["device_downloader"]
UPDATE_ALL_REVISION = DEPENDENCY_PINS["update_all"]
SUITE_SOURCE_FILES = (
    "scripts/magik_ci/delivery_tests.py",
    "scripts/magik_ci/cli.py",
    ".github/actions/verify-distribution/action.yml",
)
EVIDENCE_FORMAT = "mister-magik-delivery-evidence-v2"
ENTRYPOINTS = ("downloader", "update_all", "shipped-manager")


def expected_manager_matrix() -> list[dict[str, Any]]:
    return [
        {
            "entrypoint": "shipped-manager",
            "mode": mode,
            "allow_delete": deletion,
            "result": "passed",
            "downloader_cache": "native-executable"
            if index % 2 == 0
            else "python-archive",
        }
        for index, (mode, deletion) in enumerate(_lifecycle_settings())
    ]


def expected_update_all_matrix() -> list[dict[str, Any]]:
    return [
        {
            "entrypoint": "update_all",
            "mode": mode,
            "allow_delete": deletion,
            "result": "passed",
            "downloader_cache": "native-executable"
            if index % 2 == 0
            else "python-archive",
        }
        for index, (mode, deletion) in enumerate(
            setting
            for mode in DOWNLOADER_MODES
            for setting in ((mode, deletion) for deletion in DELETION_POLICIES)
        )
    ]


def expected_downloader_matrix() -> list[dict[str, Any]]:
    return [
        {
            "entrypoint": "downloader",
            "case": case,
            "mode": mode,
            "allow_delete": {"deletion-disabled": 0, "cores-only-deletion": 2}.get(
                case, 1
            ),
            "result": "passed",
        }
        for case in CASES
        for mode in DOWNLOADER_MODES
    ]


def evidence_for_candidate(validated: dict[str, Any]) -> dict[str, Any]:
    """Return the complete v2 evidence shape used by tests and promotion."""
    return {
        "format": EVIDENCE_FORMAT,
        "candidate_id": validated["candidate_id"],
        "suite_revision": suite_revision(),
        "dependency_pins": dict(DEPENDENCY_PINS),
        "entrypoints": list(ENTRYPOINTS),
        "settings": {
            "file_checking": list(DOWNLOADER_MODES),
            "allow_delete": list(DELETION_POLICIES),
        },
        "cases": list(CASES),
        "results": {
            "downloader": expected_downloader_matrix(),
            "update_all": expected_update_all_matrix(),
            "shipped-manager": expected_manager_matrix(),
        },
        "validation": "passed",
    }


def _lifecycle_settings():
    return (
        (mode, deletion) for mode in DOWNLOADER_MODES for deletion in DELETION_POLICIES
    )


def suite_revision() -> str:
    """Digest the gate implementation, CI action inputs, and dependency pins."""
    root = Path(__file__).resolve().parents[2]
    digest = hashlib.sha256()
    for relative in SUITE_SOURCE_FILES:
        path = root / relative
        data = path.read_bytes()
        digest.update(relative.encode())
        digest.update(len(data).to_bytes(8, "big"))
        digest.update(data)
    digest.update(json.dumps(DEPENDENCY_PINS, sort_keys=True).encode())
    digest.update(
        json.dumps(
            {"modes": DOWNLOADER_MODES, "deletion": DELETION_POLICIES, "cases": CASES},
            sort_keys=True,
        ).encode()
    )
    return digest.hexdigest()


def _results_digest(results: dict[str, Any]) -> str:
    return hashlib.sha256(
        json.dumps(results, sort_keys=True, separators=(",", ":")).encode()
    ).hexdigest()


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


def _fixture_database(
    db_id: str, files: dict[str, tuple[bytes, str]]
) -> dict[str, Any]:
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
    if (
        os.getenv("MISTER_REQUIRE_MEDIA_FAT_MOUNT") == "1"
        and not Path("/media/fat").is_mount()
    ):
        raise ValueError("balanced cache regression requires a real /media/fat mount")
    with tempfile.TemporaryDirectory(prefix="magik-cached-regression-") as temporary:
        fat = Path(temporary)
        (fat / "Scripts").mkdir(parents=True)
        (fat / "MiSTer.ini").write_text("[MiSTer]\nmain=MiSTer\n")
        requests: list[str] = []
        content: dict[str, bytes] = {}
        current = _fixture_database(
            "mister_magik",
            {"mister-magik/fixture.txt": (payload, "/mister-magik/fixture.txt")},
        )
        unrelated = _fixture_database(
            "unrelated",
            {"unrelated/fixture.txt": (other_payload, "/unrelated/fixture.txt")},
        )

        class Handler(BaseHTTPRequestHandler):
            def do_GET(self):
                path = urlsplit(self.path).path
                requests.append(path)
                data = content.get(path)
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
                base = f"http://lifecycle.example.test:{server.server_port}"
                current["files"]["mister-magik/fixture.txt"]["url"] = (
                    base + "/mister-magik/fixture.txt"
                )
                unrelated["files"]["unrelated/fixture.txt"]["url"] = (
                    base + "/unrelated/fixture.txt"
                )
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
                    mode_values = {
                        "fastest": 0,
                        "balanced": 1,
                        "exhaustive": 2,
                        "verify_integrity": 3,
                    }
                    expected_mode = mode_values[mode]
                    changed = "File checking changed from" in first
                    if (
                        mode in ("exhaustive", "verify_integrity", "fastest")
                        and changed
                    ):
                        raise ValueError(
                            f"Downloader silently changed effective mode: {mode}"
                        )
                    if (
                        mode == "balanced"
                        and changed
                        and 'from "1" to "2"' not in first
                    ):
                        raise ValueError(
                            "balanced mode changed to an unexpected effective mode"
                        )
                    if (
                        not changed
                        and expected_mode != 0
                        and mode == "balanced"
                        and os.getenv("MISTER_REQUIRE_MEDIA_FAT_MOUNT") == "1"
                    ):
                        raise ValueError(
                            "balanced mode did not resolve with a real /media/fat mount"
                        )
                    managed = fat / "mister-magik/fixture.txt"
                    if not managed.is_file():
                        raise ValueError("direct Downloader fixture was not installed")
                    requests.clear()
                    run(mode)
                    if any(path == "/mister-magik/fixture.txt" for path in requests):
                        raise ValueError(
                            f"unchanged cached run fetched payload in {mode} mode"
                        )
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
                        if any(
                            path == "/mister-magik/fixture.txt" for path in requests
                        ):
                            raise ValueError(
                                "legacy negative control unexpectedly downloaded payload"
                            )
                        if managed.exists():
                            raise ValueError(
                                "negative control did not reproduce skipped reinstall"
                            )
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
                        raise ValueError(
                            "balanced cached negative control fetched payload"
                        )
                    if managed.exists():
                        raise ValueError(
                            "balanced cached negative control unexpectedly restored payload"
                        )
            finally:
                server.shutdown()
                worker.join(timeout=5)


def downloader_failure_regression(source: Path) -> None:
    """Exercise invalid, truncated, HTTP-failure, and timeout-safe installs."""
    payload = b"failure fixture payload\n"
    database = _fixture_database(
        "mister_magik",
        {"mister-magik/failure.txt": (payload, "/mister-magik/failure.txt")},
    )
    with tempfile.TemporaryDirectory(prefix="magik-downloader-failures-") as temporary:
        root = Path(temporary)
        (root / "Scripts").mkdir()
        (root / "MiSTer.ini").write_bytes(b"[MiSTer]\nmain=MiSTer\n")
        original_ini = (root / "MiSTer.ini").read_bytes()
        state: _FailureResponse = {
            "status": 200,
            "db": json.dumps(database).encode(),
            "payload": payload,
            "delay": 0.0,
        }

        class Handler(BaseHTTPRequestHandler):
            def do_GET(self):
                path = urlsplit(self.path).path
                if state["delay"]:
                    import time

                    time.sleep(state["delay"])
                if state["status"] != 200:
                    self.send_response(state["status"])
                    self.end_headers()
                    return
                data = state["db"] if path == "/database.json" else state["payload"]
                self.send_response(200)
                self.send_header("Content-Length", str(len(data)))
                self.end_headers()
                self.wfile.write(data)

            def log_message(self, format, *args):
                pass

        with ThreadingHTTPServer(("127.0.0.1", 0), Handler) as server:
            worker = threading.Thread(target=server.serve_forever, daemon=True)
            worker.start()
            try:
                base = f"http://release.example.test:{server.server_port}"
                database["files"]["mister-magik/failure.txt"]["url"] = (
                    base + "/mister-magik/failure.txt"
                )
                state["db"] = json.dumps(database).encode()
                env = {
                    "PATH": os.environ["PATH"],
                    "DOWNLOADER_INI_PATH": str(root / "downloader.ini"),
                    "FORCED_BASE_PATH": str(root),
                    "ALLOW_REBOOT": "0",
                    "UPDATE_LINUX": "false",
                    "FAIL_ON_FILE_ERROR": "true",
                    "DEFAULT_DB_ID": "mister_magik",
                    "DEFAULT_DB_URL": base + "/database.json",
                    "HTTP_PROXY": f"http://127.0.0.1:{server.server_port}",
                    "HTTPS_PROXY": f"http://127.0.0.1:{server.server_port}",
                    "PYTHONDONTWRITEBYTECODE": "1",
                }

                def run() -> subprocess.CompletedProcess[str]:
                    # Each fault is an independent transaction.  Remove a
                    # prior failed attempt so a stale partial file cannot make
                    # a later timeout appear successful.
                    shutil.rmtree(root / "mister-magik", ignore_errors=True)
                    (root / "downloader.ini").write_text(
                        "[MiSTer]\nallow_delete = 1\nallow_reboot = 0\nupdate_linux = false\n"
                        "file_checking = exhaustive\n[mister_magik]\n"
                        f"db_url = {base}/database.json\n"
                    )
                    return subprocess.run(
                        [
                            sys.executable,
                            str(source / "src"),
                            "--run-only",
                            "mister_magik",
                        ],
                        cwd=root,
                        env=env,
                        capture_output=True,
                        text=True,
                        timeout=60,
                        check=False,
                    )

                state["status"] = 503
                result = run()
                if (
                    result.returncode == 0
                    or (root / "mister-magik/failure.txt").exists()
                ):
                    raise ValueError(
                        "HTTP failure was reported as a successful install"
                    )
                if (root / "MiSTer.ini").read_bytes() != original_ini:
                    raise ValueError("HTTP failure changed boot configuration")
                state["status"] = 200
                state["payload"] = payload[:3]
                result = run()
                if (
                    result.returncode == 0
                    or (root / "mister-magik/failure.txt").exists()
                ):
                    raise ValueError(
                        "truncated payload was reported as a successful install"
                    )
                state["payload"] = payload
                state["db"] = b"not-json"
                result = run()
                if (
                    result.returncode == 0
                    or (root / "mister-magik/failure.txt").exists()
                ):
                    raise ValueError(
                        "corrupt database was reported as a successful install"
                    )
                state["db"] = json.dumps(database).encode()
                state["delay"] = 2.0
                shutil.rmtree(root / "mister-magik", ignore_errors=True)
                (root / "downloader.ini").write_text(
                    "[MiSTer]\nallow_delete = 1\nallow_reboot = 0\nupdate_linux = false\n"
                    "downloader_timeout = 1\nfile_checking = exhaustive\n[mister_magik]\n"
                    f"db_url = {base}/database.json\n"
                )
                try:
                    result = subprocess.run(
                        [
                            sys.executable,
                            str(source / "src"),
                            "--run-only",
                            "mister_magik",
                        ],
                        cwd=root,
                        env=env,
                        capture_output=True,
                        text=True,
                        timeout=1,
                        check=False,
                    )
                except subprocess.TimeoutExpired:
                    result = None
                if result is not None and result.returncode == 0:
                    raise ValueError(
                        "timed out payload was reported as a successful install"
                    )
                if (root / "mister-magik/failure.txt").exists():
                    raise ValueError(
                        "timed out payload left a successful-looking partial install"
                    )
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


def _assert_receipt_files(root: Path, receipt: dict[str, Any]) -> None:
    """Check every published receipt path after each successful install."""
    for entry in receipt["files"]:
        path = root / entry["path"]
        if not path.is_file() or sha256_file(path) != entry["sha256"]:
            raise ValueError(f"published receipt mismatch: {entry['path']}")


def _validate_downloader_source(source: Path, revision: str, label: str) -> Path:
    source = source.resolve()
    actual = subprocess.check_output(
        ["git", "-C", str(source), "rev-parse", "HEAD"], text=True
    ).strip()
    dirty = subprocess.check_output(
        ["git", "-C", str(source), "status", "--porcelain", "--untracked-files=no"],
        text=True,
    ).strip()
    if actual != revision or dirty:
        raise ValueError(f"{label} must match the clean pinned revision")
    return source


def _validate_native_downloader(path: Path) -> Path:
    path = path.resolve()
    if not path.is_file() or path.read_bytes()[:4] != b"\x7fELF":
        raise ValueError("cached Downloader executable must be a genuine ELF runtime")
    header = path.read_bytes()[:20]
    if len(header) < 20 or header[4] != 1 or header[18:20] != b"\x28\x00":
        raise ValueError("cached Downloader executable must be an ARMv7 ELF")
    expected = os.getenv("MISTER_MAGIK_NATIVE_DOWNLOADER_SHA256")
    if not expected:
        raise ValueError("cached Downloader executable hash is not pinned by CI")
    if sha256_file(path) != expected:
        raise ValueError("cached Downloader executable hash does not match CI artifact")
    return path


def _make_downloader_archive(source: Path, destination: Path) -> None:
    """Build the executable pyz layout emitted by upstream ``src/build.sh``."""
    with tempfile.NamedTemporaryFile(prefix="downloader-pyz-", suffix=".zip") as raw:
        with zipfile.ZipFile(raw, "w", compression=zipfile.ZIP_STORED) as archive:
            source_root = source / "src"
            for path in sorted(source_root.rglob("*")):
                if path.is_file() and "__pycache__" not in path.parts:
                    archive.write(path, path.relative_to(source_root).as_posix())
        raw.flush()
        raw.seek(0)
        destination.write_bytes(b"#!/usr/bin/env python3\n" + raw.read())
    destination.chmod(0o755)


def _assert_direct_pyz(archive: Path, expected_revision: str) -> None:
    result = subprocess.run(
        [str(archive), "--version"],
        env={"PATH": os.environ["PATH"], "PYTHONDONTWRITEBYTECODE": "1"},
        capture_output=True,
        text=True,
        timeout=60,
        check=False,
    )
    if result.returncode or not result.stdout.strip().startswith("2.4"):
        raise ValueError(
            f"pinned {expected_revision} Downloader pyz did not execute directly: "
            f"{result.stdout[-500:]} {result.stderr[-500:]}"
        )


def _assert_base_and_dropin_configurations(source: Path) -> None:
    """Exercise the pinned source's two supported INI discovery layouts.

    A MagiK registration may live in the base downloader.ini or in the
    downloader_mister_magik.ini drop-in.  Keep this check offline and use the
    real pinned source so an adapter cannot accidentally require one layout.
    """
    with tempfile.TemporaryDirectory(prefix="magik-downloader-config-") as temporary:
        root = Path(temporary)
        (root / "Scripts/.config/downloader").mkdir(parents=True)
        db_url = "http://127.0.0.1/mister-magik.json"
        environment = {
            "PATH": os.environ["PATH"],
            "DOWNLOADER_INI_PATH": str(root / "downloader.ini"),
            "FORCED_BASE_PATH": str(root),
            "ALLOW_REBOOT": "0",
            "UPDATE_LINUX": "false",
            "DEFAULT_DB_ID": "mister_magik",
            "DEFAULT_DB_URL": db_url,
            "DOWNLOADER_OUTPUT": "dlp1-ltsv",
            "PYTHONDONTWRITEBYTECODE": "1",
            "HTTP_PROXY": "",
            "HTTPS_PROXY": "",
        }
        config = (
            "[MiSTer]\nallow_reboot = 0\nupdate_linux = false\n"
            "[mister_magik]\n"
            f"db_url = {db_url}\n"
        )

        def assert_configured(label: str) -> None:
            result = subprocess.run(
                [sys.executable, str(source / "src"), "--list-dbs", "configured"],
                cwd=root,
                env=environment,
                capture_output=True,
                text=True,
                timeout=60,
                check=False,
            )
            output = result.stdout + result.stderr
            if (
                result.returncode
                or "event:configured_db\tdb:mister_magik" not in output
            ):
                raise ValueError(
                    f"pinned Downloader rejected {label} configuration: {output[-1500:]}"
                )
            if "warning" in output.lower():
                raise ValueError(f"pinned Downloader warned for {label} configuration")

        # Base-only registration, as used by older/manual installations.
        (root / "downloader.ini").write_text(config)
        (root / "downloader_mister_magik.ini").unlink(missing_ok=True)
        assert_configured("base-only")

        # Drop-in registration with no base file, as emitted by the MagiK
        # installer on a stock card.  Downloader discovers the drop-in from
        # the configured base path and supplies its normal defaults.
        (root / "downloader.ini").unlink()
        (root / "downloader_mister_magik.ini").write_text(
            f"[mister_magik]\ndb_url = {db_url}\n"
        )
        assert_configured("drop-in-only")


def _empty_delivery_root(root: Path) -> None:
    for child in root.iterdir():
        if child.is_dir() and not child.is_symlink():
            shutil.rmtree(child)
        else:
            child.unlink()


def update_all_test(
    candidate: Path,
    *,
    channel: str,
    source: Path,
    device_source: Path,
    native_downloader: Path,
    update_all_source: Path,
) -> list[dict[str, Any]]:
    """Run the real Update All pyz against the candidate's local feed.

    Update All is intentionally a separate entrypoint from the direct
    Downloader matrix.  The real pyz is selected by the CI action and must be
    supplied explicitly; no downloaded or regenerated launcher is accepted.
    The Linux job provides the isolated /media/fat environment and loopback
    feed transport.  Local macOS runs keep this helper importable but do not
    claim ARM execution.
    """
    update_all_source = update_all_source.resolve()
    if not update_all_source.is_file():
        raise ValueError(f"Update All source is missing: {update_all_source}")
    if update_all_source.stat().st_size < 100:
        raise ValueError("Update All source is not a real pyz archive")
    source = _validate_downloader_source(
        source, DOWNLOADER_REVISION, "baseline Downloader"
    )
    device_source = _validate_downloader_source(
        device_source, DEVICE_DOWNLOADER_REVISION, "device-compatible Downloader"
    )
    native_downloader = _validate_native_downloader(native_downloader)
    receipt = dist.read_json(candidate / "release-assets.json")
    root = Path("/media/fat")
    if os.getenv("MISTER_MAGIK_DELIVERY_ROOT") != str(root) or not root.is_mount():
        raise ValueError("Update All gate requires an isolated /media/fat mount")
    database = dist.read_json(candidate / f"mister-magik-{channel}-db.json")
    payloads = {
        "/" + entry["asset"]: (candidate / entry["asset"]).read_bytes()
        for entry in receipt["files"]
    }
    receipt_payload_paths = set(payloads)
    receipt_hashes = {entry["path"]: entry["sha256"] for entry in receipt["files"]}
    results: list[dict[str, Any]] = []
    with tempfile.TemporaryDirectory(prefix="magik-update-all-runtime-") as temporary:
        runtime = Path(temporary)
        device_archive = runtime / "downloader-device.zip"
        baseline_archive = runtime / "downloader-baseline.zip"
        _make_downloader_archive(source, baseline_archive)
        _make_downloader_archive(device_source, device_archive)
        _assert_direct_pyz(baseline_archive, DOWNLOADER_REVISION)
        _assert_direct_pyz(device_archive, DEVICE_DOWNLOADER_REVISION)
        with _DeliveryServer(("127.0.0.1", 0), _DeliveryHandler) as server:
            server.content = payloads  # type: ignore[attr-defined]
            server.requests = []  # type: ignore[attr-defined]
            server.response_delays = {}  # type: ignore[attr-defined]
            server.response_statuses = {}  # type: ignore[attr-defined]
            worker = threading.Thread(target=server.serve_forever, daemon=True)
            worker.start()
            try:
                base = f"http://release.example.test:{server.server_port}"
                database = copy.deepcopy(database)
                for path, item in database["files"].items():
                    asset = dist.asset_name(path)
                    item["url"] = base + "/" + asset
                    payloads.setdefault("/" + asset, b"")
                payloads["/database.json"] = json.dumps(database).encode()
                legacy = b"legacy helper from the previous release\n"
                old_database = copy.deepcopy(database)
                old_database["timestamp"] = int(database.get("timestamp", 0)) - 1
                old_database["files"][dist.LEGACY_HELPER] = {
                    "url": base + "/" + dist.asset_name(dist.LEGACY_HELPER),
                    "size": len(legacy),
                    "hash": hashlib.md5(legacy).hexdigest(),
                }
                payloads["/" + dist.asset_name(dist.LEGACY_HELPER)] = legacy
                # Update All always enables its builtin database list, even
                # when no optional service is selected.  Give every builtin a
                # local empty manifest and pin the self-update URL to the
                # same loopback server so the network namespace is fail-closed.
                builtin_db_ids = (
                    "distribution_mister",
                    "jtcores",
                    "coin-opcollection/distribution-misterfpga",
                    "update_all_mister",
                    "arcade_roms_db",
                    "bios_db",
                )
                for db_id in builtin_db_ids:
                    update_all_database = _fixture_database(db_id, {})
                    update_all_path = "/" + db_id.replace("/", "-") + ".json"
                    payloads[update_all_path] = json.dumps(update_all_database).encode()

                def run_update_all(
                    mode: str,
                    deletion: int,
                    downloader_path: Path,
                    python_path: Path,
                    *,
                    reset: bool = True,
                    timeout: float = 300,
                    cancel: bool = False,
                ) -> tuple[subprocess.CompletedProcess[str], list[str]]:
                    if reset:
                        _empty_delivery_root(root)
                        dist.extract_package(candidate / receipt["archive"], root)
                        (root / "MiSTer.ini").write_text("[MiSTer]\nmain=MiSTer\n")
                    (root / "Scripts/.config/downloader").mkdir(
                        parents=True, exist_ok=True
                    )
                    config = (
                        "[MiSTer]\n"
                        f"allow_delete = {deletion}\nallow_reboot = 0\nupdate_linux = false\n"
                        f"file_checking = {mode}\n[mister_magik]\n"
                        f"db_url = {base}/database.json\n[update_all_mister]\n"
                        f"db_url = {base}/update_all_mister.json\n"
                        f"[distribution_mister]\n"
                        f"db_url = {base}/distribution_mister.json\n"
                        f"[jtcores]\n"
                        f"db_url = {base}/jtcores.json\n"
                        f"[coin-opcollection/distribution-misterfpga]\n"
                        f"db_url = {base}/coin-opcollection-distribution-misterfpga.json\n"
                        f"[arcade_roms_db]\n"
                        f"db_url = {base}/arcade_roms_db.json\n"
                        f"[bios_db]\n"
                        f"db_url = {base}/bios_db.json\n"
                    )
                    # Update All reads the canonical device path while the
                    # shipped manager's bounded adapter reads the legacy root
                    # override.  Keep both views identical for this isolated
                    # lifecycle only.
                    (root / "Scripts/.config/downloader/downloader.ini").write_text(
                        config
                    )
                    (root / "downloader.ini").write_text(config)
                    cached_zip = (
                        root / "Scripts/.config/downloader/downloader_latest.zip"
                    )
                    cached_bin = root / "Scripts/.config/downloader/downloader_bin"
                    if downloader_path == cached_zip:
                        shutil.copy2(device_archive, cached_zip)
                        cached_bin.unlink(missing_ok=True)
                        effective_downloader_path = cached_zip
                    else:
                        shutil.copy2(native_downloader, cached_bin)
                        cached_bin.chmod(0o755)
                        cached_zip.unlink(missing_ok=True)
                        effective_downloader_path = cached_bin
                    start = len(server.requests)  # type: ignore[attr-defined]
                    env = {
                        "PATH": os.environ["PATH"],
                        "LANG": os.environ.get("LANG", "en_US.UTF-8"),
                        "LOCATION_STR": "mister",
                        "UPDATE_ALL_NON_INTERACTIVE": "true",
                        "SKIP_DOWNLOADER": "false",
                        "UPDATE_ALL_DOWNLOADER_PATH": str(effective_downloader_path),
                        "UPDATE_ALL_DOWNLOADER_PYTHON_COMPATIBLE_PATH": str(
                            python_path
                        ),
                        "HTTP_PROXY": f"http://127.0.0.1:{server.server_port}",
                        "HTTPS_PROXY": f"http://127.0.0.1:{server.server_port}",
                        "RETROACCOUNT_DOMAIN": base + "/retroaccount",
                        "UPDATE_ALL_MISTER_DB_URL": base + "/update_all_mister.json",
                        "PYTHONDONTWRITEBYTECODE": "1",
                    }
                    process = subprocess.Popen(
                        [sys.executable, str(update_all_source), "--no-continue"],
                        cwd=root,
                        env=env,
                        stdout=subprocess.PIPE,
                        stderr=subprocess.PIPE,
                        text=True,
                        start_new_session=True,
                    )
                    if cancel:
                        import time

                        time.sleep(0.5)
                        try:
                            os.killpg(process.pid, signal.SIGINT)
                        except ProcessLookupError:
                            pass
                        try:
                            process.communicate(timeout=10)
                        except subprocess.TimeoutExpired:
                            os.killpg(process.pid, signal.SIGTERM)
                            process.communicate(timeout=10)
                        raise ValueError("Update All cancelled by lifecycle test")
                    try:
                        stdout, stderr = process.communicate(timeout=timeout)
                    except subprocess.TimeoutExpired as error:
                        # A cancelled wrapper must not leave a Downloader
                        # child behind in the delivery namespace.
                        os.killpg(process.pid, signal.SIGTERM)
                        stdout, stderr = process.communicate(timeout=10)
                        raise ValueError(
                            f"Update All timed out after {timeout:g} seconds"
                        ) from error
                    result = subprocess.CompletedProcess(
                        process.args, process.returncode, stdout, stderr
                    )
                    requests = list(server.requests[start:])  # type: ignore[attr-defined]
                    if result.returncode:
                        raise ValueError(
                            f"real Update All {channel} lifecycle failed ({mode}/{deletion}): "
                            f"{result.stdout[-2500:]} {result.stderr[-1500:]}"
                        )
                    unexpected = [
                        path for path in requests if urlsplit(path).path not in payloads
                    ]
                    if unexpected:
                        raise ValueError(
                            "Update All made an unexpected external request: "
                            + ",".join(unexpected[:8])
                        )
                    expected = {
                        entry["path"]: entry["sha256"] for entry in receipt["files"]
                    }
                    for path, digest in expected.items():
                        installed = root / path
                        if not installed.is_file() or sha256_file(installed) != digest:
                            raise ValueError(
                                f"Update All changed/missed payload: {path}"
                            )
                    return result, requests

                for index, (mode, deletion) in enumerate(_lifecycle_settings()):
                    cached_bin = native_downloader
                    cached_zip = (
                        root / "Scripts/.config/downloader/downloader_latest.zip"
                    )
                    downloader_path = cached_bin if index % 2 == 0 else cached_zip
                    python_path = Path(sys.executable)
                    payloads["/database.json"] = json.dumps(old_database).encode()
                    _, _first_requests = run_update_all(
                        mode, deletion, downloader_path, python_path
                    )
                    legacy_path = root / dist.LEGACY_HELPER
                    if not legacy_path.is_file():
                        raise ValueError(
                            "Update All old-helper fixture was not installed"
                        )
                    payloads["/database.json"] = json.dumps(database).encode()
                    _, second_requests = run_update_all(
                        mode, deletion, downloader_path, python_path, reset=False
                    )
                    if deletion == 1 and legacy_path.exists():
                        raise ValueError(
                            "Update All allow_delete=1 retained old helper"
                        )
                    if deletion != 1 and not legacy_path.exists():
                        raise ValueError(
                            f"Update All allow_delete={deletion} removed old helper"
                        )
                    asset_requests = [
                        path
                        for path in second_requests
                        if urlsplit(path).path in receipt_payload_paths
                    ]
                    if asset_requests:
                        raise ValueError(
                            f"Update All unchanged run fetched payload ({mode}/{deletion})"
                        )
                    original_ini = (root / "MiSTer.ini").read_bytes()
                    manager_env: dict[str, str] = {
                        **os.environ,
                        "PATH": os.environ["PATH"],
                        "MISTER_MAGIK_FAT": str(root),
                        "MISTER_MAGIK_INITTAB": str(root / "test-inittab"),
                        "MISTER_MAGIK_TEST_MODE": "1",
                        "DOWNLOADER_INI_PATH": str(
                            root / "Scripts/.config/downloader/downloader.ini"
                        ),
                        "FORCED_BASE_PATH": str(root),
                        "ALLOW_REBOOT": "0",
                        "UPDATE_LINUX": "false",
                        "HTTP_PROXY": f"http://127.0.0.1:{server.server_port}",
                        "HTTPS_PROXY": f"http://127.0.0.1:{server.server_port}",
                        "DEFAULT_DB_ID": "mister_magik",
                        "DEFAULT_DB_URL": base + "/database.json",
                        "FAIL_ON_FILE_ERROR": "true",
                        "PYTHONDONTWRITEBYTECODE": "1",
                    }
                    (root / "test-inittab").write_text(
                        "::sysinit:/media/fat/MiSTer &\n"
                    )
                    # Do not force the manager's override path.  The test must
                    # prove that the adapter selects the cached executable on
                    # alternating rows and the cached Python archive on the
                    # remaining rows.
                    manager_env.pop("MISTER_MAGIK_DOWNLOADER", None)
                    commands: tuple[tuple[str, str], ...] = (
                        ("install", "down"),
                        ("restore", "other"),
                        ("uninstall", "down,other"),
                    )
                    for command, keys in commands:
                        manager = subprocess.run(
                            ["/bin/sh", str(root / dist.LAUNCHER), command],
                            cwd=root,
                            env={**manager_env, "MISTER_MAGIK_TEST_KEYS": keys},
                            capture_output=True,
                            text=True,
                            timeout=180,
                            check=False,
                        )
                        if manager.returncode:
                            raise ValueError(
                                f"shipped manager {command} failed ({mode}/{deletion}): "
                                f"{manager.stdout[-1500:]} {manager.stderr[-1500:]}"
                            )
                    if (root / "MiSTer.ini").read_bytes() != original_ini:
                        raise ValueError(
                            "Update All lifecycle did not restore original INI"
                        )
                    if (root / "mister-magik").exists():
                        raise ValueError(
                            "Update All lifecycle left package files after uninstall"
                        )
                    # Recreate only the Downloader configuration section; the
                    # payload and registration must come from the final real
                    # Update All run, never from a manual reseed.
                    (root / "Scripts/.config/downloader").mkdir(
                        parents=True, exist_ok=True
                    )
                    (root / "Scripts/.config/downloader/downloader.ini").write_text(
                        "[MiSTer]\n"
                        f"allow_delete = {deletion}\nallow_reboot = 0\nupdate_linux = false\n"
                        f"file_checking = {mode}\n[mister_magik]\n"
                        f"db_url = {base}/database.json\n"
                    )
                    final_bin = native_downloader
                    final_requests_before = len(server.requests)  # type: ignore[attr-defined]
                    run_update_all(mode, deletion, final_bin, python_path, reset=False)
                    final_requests = list(server.requests[final_requests_before:])  # type: ignore[attr-defined]
                    _assert_receipt_files(root, receipt)
                    if not any(
                        urlsplit(path).path in receipt_payload_paths
                        for path in final_requests
                    ):
                        raise ValueError(
                            "fixed uninstaller did not force a real same-feed reinstall"
                        )
                    smoke(root)
                    results.append(
                        {
                            "entrypoint": "update_all",
                            "mode": mode,
                            "allow_delete": deletion,
                            "result": "passed",
                            "downloader_cache": "native-executable"
                            if index % 2 == 0
                            else "python-archive",
                            "effective_file_checking": mode,
                            "effective_allow_delete": deletion,
                            "receipt_hashes": receipt_hashes,
                            "unchanged_payload_requests": 0,
                            "reinstall_payload_requests": len(
                                [
                                    path
                                    for path in final_requests
                                    if urlsplit(path).path in receipt_payload_paths
                                ]
                            ),
                        }
                    )
                # Real Update All fault boundaries: a failed database fetch,
                # malformed manifest, and wrapper cancellation each preserve
                # the boot file/package and can retry with the same cache.
                fault_mode, fault_deletion = "exhaustive", 1
                run_update_all(
                    fault_mode,
                    fault_deletion,
                    native_downloader,
                    Path(sys.executable),
                )
                fault_ini = (root / "MiSTer.ini").read_bytes()

                payloads["/database.json"] = b"not-json"
                try:
                    run_update_all(
                        fault_mode,
                        fault_deletion,
                        native_downloader,
                        Path(sys.executable),
                        reset=False,
                    )
                except ValueError:
                    pass
                else:
                    raise ValueError("Update All accepted a corrupt manifest")
                if (root / "MiSTer.ini").read_bytes() != fault_ini:
                    raise ValueError(
                        "corrupt Update All manifest changed boot configuration"
                    )
                _assert_receipt_files(root, receipt)

                payloads["/database.json"] = json.dumps(database).encode()
                run_update_all(
                    fault_mode,
                    fault_deletion,
                    native_downloader,
                    Path(sys.executable),
                    reset=False,
                )

                server.response_statuses["/database.json"] = 503  # type: ignore[attr-defined]
                try:
                    run_update_all(
                        fault_mode,
                        fault_deletion,
                        native_downloader,
                        Path(sys.executable),
                        reset=False,
                    )
                except ValueError:
                    pass
                else:
                    raise ValueError("Update All accepted an HTTP database failure")
                if (root / "MiSTer.ini").read_bytes() != fault_ini:
                    raise ValueError(
                        "HTTP-failed Update All changed boot configuration"
                    )
                server.response_statuses.clear()  # type: ignore[attr-defined]
                run_update_all(
                    fault_mode,
                    fault_deletion,
                    native_downloader,
                    Path(sys.executable),
                    reset=False,
                )

                server.response_delays["/database.json"] = 5.0  # type: ignore[attr-defined]
                try:
                    run_update_all(
                        fault_mode,
                        fault_deletion,
                        native_downloader,
                        Path(sys.executable),
                        reset=False,
                        timeout=1,
                    )
                except ValueError as error:
                    if "timed out" not in str(error):
                        raise
                else:
                    raise ValueError("cancelled Update All unexpectedly succeeded")
                if (root / "MiSTer.ini").read_bytes() != fault_ini:
                    raise ValueError("cancelled Update All changed boot configuration")
                server.response_delays.clear()  # type: ignore[attr-defined]
                run_update_all(
                    fault_mode,
                    fault_deletion,
                    native_downloader,
                    Path(sys.executable),
                    reset=False,
                )
                # Deliberate SIGINT cancellation is distinct from the bounded
                # timeout above.  Retained state, boot bytes, and unrelated
                # files must survive; a normal retry then completes.
                unrelated = root / "Scripts/user-owned-delivery-file"
                unrelated.parent.mkdir(parents=True, exist_ok=True)
                unrelated.write_bytes(b"unrelated delivery state\n")
                server.response_delays["/database.json"] = 5.0  # type: ignore[attr-defined]
                try:
                    run_update_all(
                        fault_mode,
                        fault_deletion,
                        native_downloader,
                        Path(sys.executable),
                        reset=False,
                        cancel=True,
                    )
                except ValueError as error:
                    if "cancelled" not in str(error):
                        raise
                else:
                    raise ValueError(
                        "SIGINT-cancelled Update All unexpectedly succeeded"
                    )
                if (
                    root / "MiSTer.ini"
                ).read_bytes() != fault_ini or not unrelated.is_file():
                    raise ValueError(
                        "Update All cancellation changed retained lifecycle state"
                    )
                _assert_receipt_files(root, receipt)
                server.response_delays.clear()  # type: ignore[attr-defined]
                run_update_all(
                    fault_mode,
                    fault_deletion,
                    native_downloader,
                    Path(sys.executable),
                    reset=False,
                )
                if not unrelated.is_file():
                    raise ValueError("Update All retry removed unrelated user state")
            finally:
                server.shutdown()
                worker.join(timeout=5)
    return results


def shipped_manager_lifecycle_test(
    candidate: Path,
    *,
    channel: str,
    source: Path,
    device_source: Path,
    native_downloader: Path,
) -> list[dict[str, Any]]:
    """Run the shipped ARM manager through install/restore/uninstall/reinstall.

    The matrix deliberately uses the same byte-identical release feed for the
    final download.  A manual reseed is not used: the initial download creates
    the real Downloader registration, the manager delegates its removal, and
    the final Downloader run must discover and fetch the now-unregistered
    files.  QEMU/binfmt and the ARM manager are supplied by the Linux action.
    """
    source = source.resolve()
    device_source = _validate_downloader_source(
        device_source, DEVICE_DOWNLOADER_REVISION, "device-compatible Downloader"
    )
    native_downloader = _validate_native_downloader(native_downloader)
    receipt = dist.read_json(candidate / "release-assets.json")
    database = dist.read_json(candidate / f"mister-magik-{channel}-db.json")
    payloads = {
        "/" + entry["asset"]: (candidate / entry["asset"]).read_bytes()
        for entry in receipt["files"]
    }
    receipt_payload_paths = set(payloads)
    receipt_hashes = {entry["path"]: entry["sha256"] for entry in receipt["files"]}
    results: list[dict[str, Any]] = []
    runtime_dir = tempfile.TemporaryDirectory(prefix="magik-manager-runtime-")
    runtime = Path(runtime_dir.name)
    baseline_archive = runtime / "downloader-baseline.zip"
    device_archive = runtime / "downloader-device.zip"
    _make_downloader_archive(source, baseline_archive)
    _make_downloader_archive(device_source, device_archive)
    _assert_direct_pyz(baseline_archive, DOWNLOADER_REVISION)
    _assert_direct_pyz(device_archive, DEVICE_DOWNLOADER_REVISION)
    with _DeliveryServer(("127.0.0.1", 0), _DeliveryHandler) as server:
        server.content = payloads  # type: ignore[attr-defined]
        server.requests = []  # type: ignore[attr-defined]
        server.response_delays = {}  # type: ignore[attr-defined]
        server.response_statuses = {}  # type: ignore[attr-defined]
        worker = threading.Thread(target=server.serve_forever, daemon=True)
        worker.start()
        try:
            base = f"http://release.example.test:{server.server_port}"
            database = copy.deepcopy(database)
            for path, item in database["files"].items():
                item["url"] = base + "/" + dist.asset_name(path)
                payloads["/" + dist.asset_name(path)] = payloads.pop(
                    "/" + dist.asset_name(path), b""
                )
            payloads["/database.json"] = json.dumps(database).encode()
            legacy = b"legacy helper from the previous release\n"
            old_database = copy.deepcopy(database)
            old_database["timestamp"] = int(database.get("timestamp", 0)) - 1
            old_database["files"][dist.LEGACY_HELPER] = {
                "url": base + "/" + dist.asset_name(dist.LEGACY_HELPER),
                "size": len(legacy),
                "hash": hashlib.md5(legacy).hexdigest(),
            }
            payloads["/" + dist.asset_name(dist.LEGACY_HELPER)] = legacy
            for index, (mode, deletion) in enumerate(_lifecycle_settings()):
                with tempfile.TemporaryDirectory(
                    prefix="magik-manager-lifecycle-"
                ) as temporary:
                    root = Path(temporary)
                    dist.extract_package(candidate / receipt["archive"], root)
                    (root / "MiSTer.ini").write_text("[MiSTer]\nmain=MiSTer\n")
                    (root / "test-inittab").write_text(
                        "::sysinit:/media/fat/MiSTer &\n"
                    )
                    (root / "downloader.ini").write_text(
                        "[MiSTer]\n"
                        f"allow_delete = {deletion}\nallow_reboot = 0\nupdate_linux = false\n"
                        f"file_checking = {mode}\n[mister_magik]\n"
                        f"db_url = {base}/database.json\n"
                    )
                    cache_dir = root / "Scripts/.config/downloader"
                    cache_dir.mkdir(parents=True, exist_ok=True)
                    cache = baseline_archive if index % 2 == 0 else device_archive
                    if index % 2 == 0:
                        shutil.copy2(native_downloader, cache_dir / "downloader_bin")
                        (cache_dir / "downloader_bin").chmod(0o755)
                    else:
                        shutil.copy2(cache, cache_dir / "downloader_latest.zip")
                    (root / "downloader.ini").write_text(
                        "[MiSTer]\n"
                        f"allow_delete = {deletion}\nallow_reboot = 0\nupdate_linux = false\n"
                        f"file_checking = {mode}\n[mister_magik]\n"
                        f"db_url = {base}/database.json\n"
                    )
                    env = {
                        **os.environ,
                        "PATH": os.environ["PATH"],
                        "MISTER_MAGIK_FAT": str(root),
                        "MISTER_MAGIK_INITTAB": str(root / "test-inittab"),
                        "MISTER_MAGIK_TEST_MODE": "1",
                        "DOWNLOADER_INI_PATH": str(root / "downloader.ini"),
                        "FORCED_BASE_PATH": str(root),
                        "ALLOW_REBOOT": "0",
                        "UPDATE_LINUX": "false",
                        "HTTP_PROXY": f"http://127.0.0.1:{server.server_port}",
                        "HTTPS_PROXY": f"http://127.0.0.1:{server.server_port}",
                        "DEFAULT_DB_ID": "mister_magik",
                        "DEFAULT_DB_URL": base + "/database.json",
                        "FAIL_ON_FILE_ERROR": "true",
                        "PYTHONDONTWRITEBYTECODE": "1",
                    }

                    def run_manager(
                        command: str,
                        keys: str,
                        root=root,
                        env=env,
                        mode=mode,
                        deletion=deletion,
                    ) -> subprocess.CompletedProcess[str]:
                        result = subprocess.run(
                            ["/bin/sh", str(root / dist.LAUNCHER), command],
                            cwd=root,
                            env={**env, "MISTER_MAGIK_TEST_KEYS": keys},
                            capture_output=True,
                            text=True,
                            timeout=180,
                            check=False,
                        )
                        if result.returncode:
                            raise ValueError(
                                f"shipped manager {command} failed ({mode}/{deletion}): "
                                f"{result.stdout[-1500:]} {result.stderr[-1500:]}"
                            )
                        return result

                    server.requests.clear()  # type: ignore[attr-defined]
                    payloads["/database.json"] = json.dumps(old_database).encode()
                    initial = _run_delivery_downloader(
                        source, root, env, mode, base, deletion
                    )
                    if initial.returncode:
                        raise ValueError("initial real Downloader install failed")
                    _assert_receipt_files(root, receipt)
                    legacy_path = root / dist.LEGACY_HELPER
                    if not legacy_path.is_file():
                        raise ValueError("old-helper fixture was not installed")
                    payloads["/database.json"] = json.dumps(database).encode()
                    current = _run_delivery_downloader(
                        source, root, env, mode, base, deletion
                    )
                    if current.returncode:
                        raise ValueError("current real Downloader install failed")
                    _assert_receipt_files(root, receipt)
                    if deletion == 1 and legacy_path.exists():
                        raise ValueError("allow_delete=1 retained old helper")
                    if deletion != 1 and not legacy_path.exists():
                        raise ValueError(f"allow_delete={deletion} removed old helper")
                    unchanged_start = len(server.requests)  # type: ignore[attr-defined]
                    unchanged = _run_delivery_downloader(
                        source, root, env, mode, base, deletion
                    )
                    if unchanged.returncode:
                        raise ValueError("unchanged real Downloader run failed")
                    if any(
                        urlsplit(path).path in receipt_payload_paths
                        for path in server.requests[unchanged_start:]  # type: ignore[attr-defined]
                    ):
                        raise ValueError(
                            f"unchanged run fetched payload ({mode}/{deletion})"
                        )
                    original_ini = (root / "MiSTer.ini").read_bytes()
                    run_manager("install", "down")
                    run_manager("restore", "other")
                    if (root / "MiSTer.ini").read_bytes() != original_ini:
                        raise ValueError("restore did not preserve the original INI")
                    reinstall_start = len(server.requests)  # type: ignore[attr-defined]
                    run_manager("uninstall", "down,other")
                    if (root / "mister-magik").exists():
                        raise ValueError(
                            "shipped manager left package files after uninstall"
                        )
                    if (root / "MiSTer.ini").read_bytes() != original_ini:
                        raise ValueError("uninstall did not preserve restored INI")
                    reinstall = _run_delivery_downloader(
                        source, root, env, mode, base, deletion
                    )
                    if reinstall.returncode:
                        raise ValueError("same-version Downloader reinstall failed")
                    _assert_receipt_files(root, receipt)
                    if not any(
                        urlsplit(path).path in receipt_payload_paths
                        for path in server.requests[reinstall_start:]  # type: ignore[attr-defined]
                    ):
                        raise ValueError(
                            "fixed uninstaller did not force a real same-feed reinstall"
                        )
                    smoke(root)
                    results.append(
                        {
                            "entrypoint": "shipped-manager",
                            "mode": mode,
                            "allow_delete": deletion,
                            "result": "passed",
                            "downloader_cache": "native-executable"
                            if index % 2 == 0
                            else "python-archive",
                            "effective_file_checking": mode,
                            "effective_allow_delete": deletion,
                            "receipt_hashes": receipt_hashes,
                            "unchanged_payload_requests": 0,
                            "reinstall_payload_requests": len(
                                [
                                    path
                                    for path in server.requests[reinstall_start:]  # type: ignore[attr-defined]
                                    if urlsplit(path).path in receipt_payload_paths
                                ]
                            ),
                        }
                    )
        finally:
            server.shutdown()
            worker.join(timeout=5)
    runtime_dir.cleanup()
    return results


class _DeliveryHandler(BaseHTTPRequestHandler):
    server: _DeliveryServer

    def do_GET(self):
        path = urlsplit(self.path).path
        self.server.requests.append(path)  # type: ignore[attr-defined]
        log_path = os.getenv("MISTER_MAGIK_DELIVERY_HTTP_LOG")
        if log_path:
            with Path(log_path).open("a") as log:
                log.write(path + "\n")
        delay = self.server.response_delays.get(path, 0.0)  # type: ignore[attr-defined]
        if delay:
            import time

            time.sleep(delay)
        status = self.server.response_statuses.get(path, 200)  # type: ignore[attr-defined]
        data = self.server.content.get(path, b"")  # type: ignore[attr-defined]
        self.send_response(status if path in self.server.content else 404)
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        if path in self.server.content and status == 200:
            self.wfile.write(data)

    def log_message(self, format, *args):
        pass


def _run_delivery_downloader(
    source: Path,
    root: Path,
    env: dict[str, str],
    mode: str,
    base: str,
    deletion: int,
) -> subprocess.CompletedProcess[str]:
    (root / "downloader.ini").write_text(
        f"[MiSTer]\nallow_delete = {deletion}\nallow_reboot = 0\nupdate_linux = false\n"
        f"file_checking = {mode}\n[mister_magik]\ndb_url = {base}/database.json\n"
    )
    return subprocess.run(
        [sys.executable, str(source / "src"), "--run-only", "mister_magik"],
        cwd=root,
        env=env,
        capture_output=True,
        text=True,
        timeout=180,
        check=False,
    )


def downloader_test(
    candidate: Path, *, channel: str, source: Path, run_smoke: bool = True
) -> list[dict[str, Any]]:
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
    _assert_base_and_dropin_configurations(source)
    direct_cached_reinstall_regression(source)
    downloader_failure_regression(source)
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
    candidate_payload_paths = {"/" + entry["asset"] for entry in receipt["files"]}
    requests: list[str] = []
    results: list[dict[str, Any]] = []

    class Handler(BaseHTTPRequestHandler):
        def do_GET(self):
            path = urlsplit(self.path).path
            requests.append(path)
            data = content.get(path)
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

                    def download(
                        database,
                        mode="balanced",
                        fat=fat,
                        environment=environment,
                        case=case,
                        deletion=deletion,
                    ):
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
                        unchanged_start = len(requests)
                        unchanged_output = download(current, mode)
                        if any(
                            path in candidate_payload_paths
                            for path in requests[unchanged_start:]
                        ):
                            raise ValueError(
                                f"unchanged run fetched payload in {mode}/{case}: "
                                f"{unchanged_output[-500:]}"
                            )
                        if (fat / dist.LEGACY_HELPER).exists() != (deletion != 1):
                            raise ValueError(
                                f"Downloader deletion policy mismatch: {case}"
                            )
                        if {
                            name: (fat / name).read_bytes() for name in protected
                        } != protected:
                            raise ValueError("Downloader changed user/boot files")
                        if run_smoke:
                            smoke(fat)
                        results.append(
                            {
                                "entrypoint": "downloader",
                                "case": case,
                                "mode": mode,
                                "allow_delete": deletion,
                                "result": "passed",
                                "effective_file_checking": mode,
                                "effective_allow_delete": deletion,
                                "unchanged_payload_requests": 0,
                            }
                        )
        finally:
            server.shutdown()
            worker.join(timeout=5)
    return results


def _write_failure_artifact(error: Exception) -> None:
    runner_temp = os.getenv("RUNNER_TEMP")
    if not runner_temp:
        return
    artifact = Path(runner_temp) / "mister-magik-delivery-failure.log"
    atomic_write(artifact, (str(error) + "\n").encode())


def run(
    candidate: Path,
    *,
    channel: str,
    source: Path,
    device_source: Path,
    native_downloader: Path,
    update_all_source: Path | None = None,
) -> dict[str, Any]:
    try:
        return _run_delivery(
            candidate,
            channel=channel,
            source=source,
            device_source=device_source,
            native_downloader=native_downloader,
            update_all_source=update_all_source,
        )
    except Exception as error:
        _write_failure_artifact(error)
        raise


def _run_delivery(
    candidate: Path,
    *,
    channel: str,
    source: Path,
    device_source: Path,
    native_downloader: Path,
    update_all_source: Path | None = None,
) -> dict[str, Any]:
    candidate = candidate.resolve()
    if update_all_source is None:
        raise ValueError("complete delivery evidence requires --update-all-source")
    validated = dist.verify(candidate, channel=channel)
    receipt = dist.read_json(candidate / "release-assets.json")
    with tempfile.TemporaryDirectory(prefix="magik-shipped-installer-") as temporary:
        root = Path(temporary)
        dist.extract_package(candidate / receipt["archive"], root / "zip")
        dist.reconstruct(candidate, channel, receipt, root / "downloaded")
        smoke(root / "zip")
        smoke(root / "downloaded")
    downloader_result = downloader_test(candidate, channel=channel, source=source)
    manager_matrix = shipped_manager_lifecycle_test(
        candidate,
        channel=channel,
        source=source,
        device_source=device_source,
        native_downloader=native_downloader,
    )
    update_all_result = update_all_test(
        candidate,
        channel=channel,
        source=source,
        device_source=device_source,
        native_downloader=native_downloader,
        update_all_source=update_all_source,
    )
    evidence = evidence_for_candidate(validated)
    evidence["results"]["downloader"] = downloader_result
    evidence["results"]["shipped-manager"] = manager_matrix
    evidence["results"]["update_all"] = update_all_result
    evidence["execution"] = {
        "status": "passed",
        "candidate_id": validated["candidate_id"],
        "result_digest": _results_digest(evidence["results"]),
    }
    atomic_write(candidate / dist.EVIDENCE, dist.canonical_json(evidence))
    dist.write_checksums(candidate)
    return evidence


def require_evidence(candidate: Path, validated: dict[str, Any]) -> None:
    try:
        actual = dist.read_json(candidate / dist.EVIDENCE)
    except (OSError, ValueError, TypeError, json.JSONDecodeError) as error:
        raise ValueError(
            "complete delivery evidence is missing or unreadable"
        ) from error
    if actual.get("format") != EVIDENCE_FORMAT:
        raise ValueError("v1 or unknown delivery evidence is not accepted")
    expected = evidence_for_candidate(validated)
    for field in (
        "candidate_id",
        "suite_revision",
        "dependency_pins",
        "entrypoints",
        "settings",
        "cases",
    ):
        if actual.get(field) != expected.get(field):
            raise ValueError(
                "complete passing delivery evidence for this exact candidate is required"
            )
    execution = actual.get("execution")
    if not isinstance(execution, dict) or execution.get("status") != "passed":
        raise ValueError(
            "complete passing delivery evidence must record an executed suite"
        )
    if execution.get("candidate_id") != validated["candidate_id"]:
        raise ValueError("delivery execution is bound to a different candidate")
    results = actual.get("results")
    if not isinstance(results, dict) or execution.get(
        "result_digest"
    ) != _results_digest(results):
        raise ValueError(
            "complete passing delivery execution results are missing or stale"
        )
    downloader = results.get("downloader")
    if not isinstance(downloader, list) or len(downloader) != len(CASES) * len(
        DOWNLOADER_MODES
    ):
        raise ValueError("complete passing Downloader matrix is required")
    expected_keys = {
        (case, mode, deletion)
        for case in CASES
        for mode in DOWNLOADER_MODES
        for deletion in DELETION_POLICIES
        if deletion == {"deletion-disabled": 0, "cores-only-deletion": 2}.get(case, 1)
    }
    actual_keys = {
        (row.get("case"), row.get("mode"), row.get("allow_delete"))
        for row in downloader
        if isinstance(row, dict)
    }
    if actual_keys != expected_keys or any(
        row.get("result") != "passed" for row in downloader
    ):
        raise ValueError("complete passing Downloader matrix is required")
    for name in ("update_all", "shipped-manager"):
        matrix = results.get(name)
        expected_matrix = (
            expected_update_all_matrix()
            if name == "update_all"
            else expected_manager_matrix()
        )
        if not isinstance(matrix, list) or len(matrix) != len(expected_matrix):
            raise ValueError(f"complete passing {name} matrix is required")
        actual_matrix_keys = {
            (row.get("entrypoint"), row.get("mode"), row.get("allow_delete"))
            for row in matrix
            if isinstance(row, dict)
        }
        expected_matrix_keys = {
            (row["entrypoint"], row["mode"], row["allow_delete"])
            for row in expected_matrix
        }
        if actual_matrix_keys != expected_matrix_keys or any(
            row.get("result") != "passed" for row in matrix
        ):
            raise ValueError(f"complete passing {name} matrix is required")
