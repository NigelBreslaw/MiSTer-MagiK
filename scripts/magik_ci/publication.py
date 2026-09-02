# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

"""Immutable release publication and atomic channel-feed promotion through gh."""

from __future__ import annotations

import base64
import json
import re
import subprocess
import tempfile
from pathlib import Path
from typing import Any

from . import delivery_tests, downloader_db
from . import distribution as dist
from .common import sha256_file


class GitHub:
    def __init__(self, repository: str):
        if not re.fullmatch(r"[\w.-]+/[\w.-]+", repository):
            raise ValueError("invalid release repository")
        self.repository = repository

    def command(self, args: list[str]) -> str:
        result = subprocess.run(
            ["gh", *args], capture_output=True, text=True, check=True, timeout=300
        )
        return result.stdout

    def api(
        self, path: str, data: dict[str, Any] | None = None, *, method: str = "GET"
    ) -> Any:
        args = ["gh", "api", "--method", method, f"repos/{self.repository}/{path}"]
        if data is not None:
            args += ["--input", "-"]
        result = subprocess.run(
            args,
            input=json.dumps(data) if data is not None else None,
            capture_output=True,
            text=True,
            check=True,
            timeout=60,
        )
        return json.loads(result.stdout)

    def optional(self, path: str) -> Any:
        try:
            return self.api(path)
        except subprocess.CalledProcessError as error:
            if "HTTP 404" in (error.stderr or ""):
                return None
            raise

    def release(self, tag: str) -> Any:
        return self.optional(f"releases/tags/{tag}")

    def revision(self, tag: str) -> str:
        return self.api(f"commits/{tag}")["sha"]

    def create(self, tag: str, revision: str) -> None:
        self.command(
            [
                "release",
                "create",
                tag,
                "--repo",
                self.repository,
                "--target",
                revision,
                "--title",
                f"MiSTer MagiK {tag}",
                "--notes",
                "Validated immutable MiSTer MagiK payload.",
                "--prerelease",
            ]
        )

    def upload(self, tag: str, paths: list[Path], *, replace: bool = False) -> None:
        if not paths:
            return
        if replace and (
            tag not in dist.CHANNELS
            or any(
                not re.fullmatch(
                    r"mister-magik-(alpha|beta|release)-(db\.json(\.zip)?|installer\.zip)",
                    path.name,
                )
                for path in paths
            )
        ):
            raise ValueError("only channel feed/bootstrap files may be replaced")
        self.command(
            [
                "release",
                "upload",
                tag,
                *map(str, paths),
                "--repo",
                self.repository,
                *(["--clobber"] if replace else []),
            ]
        )

    def download(self, tag: str, destination: Path, names: list[str]) -> None:
        if names:
            self.command(
                [
                    "release",
                    "download",
                    tag,
                    "--repo",
                    self.repository,
                    "--dir",
                    str(destination),
                    *(arg for name in names for arg in ("--pattern", name)),
                ]
            )

    def presentation(self, tag: str, version: str, *, stable: bool = False) -> None:
        self.command(
            [
                "release",
                "edit",
                tag,
                "--repo",
                self.repository,
                "--title",
                f"MiSTer MagiK {version} {tag if tag in dist.CHANNELS else ''}".strip(),
                "--notes",
                f"Validated payload: https://github.com/{self.repository}/releases/tag/v{version}. Channel files reference immutable assets; legacy assets are retained.",
                f"--prerelease={'false' if stable else 'true'}",
            ]
        )

    def feed(self, channel: str) -> bytes | None:
        item = self.optional(
            f"contents/mister-magik-{channel}-db.json.zip?ref=downloader"
        )
        return base64.b64decode(item["content"]) if item is not None else None

    def update_feed(self, files: dict[str, bytes], version: str) -> None:
        reference = self.optional("git/ref/heads/downloader")
        parent = reference["object"]["sha"] if reference else None
        tree_data: dict[str, Any] = {"tree": []}
        if parent:
            tree_data["base_tree"] = self.api(f"git/commits/{parent}")["tree"]["sha"]
        for name, value in sorted(files.items()):
            if not re.fullmatch(
                r"mister-magik-(alpha|beta|release)-db\.json\.zip", name
            ):
                raise ValueError("unexpected channel feed path")
            blob = self.api(
                "git/blobs",
                {"content": base64.b64encode(value).decode(), "encoding": "base64"},
                method="POST",
            )
            tree_data["tree"].append(
                {"path": name, "mode": "100644", "type": "blob", "sha": blob["sha"]}
            )
        tree = self.api("git/trees", tree_data, method="POST")
        commit = self.api(
            "git/commits",
            {
                "message": f"Publish verified MiSTer MagiK {version} channel feeds",
                "tree": tree["sha"],
                "parents": [parent] if parent else [],
            },
            method="POST",
        )
        if parent:
            self.api(
                "git/refs/heads/downloader",
                {"sha": commit["sha"], "force": False},
                method="PATCH",
            )
        else:
            self.api(
                "git/refs",
                {"ref": "refs/heads/downloader", "sha": commit["sha"]},
                method="POST",
            )
        for name, value in files.items():
            channel = name.removeprefix("mister-magik-").removesuffix("-db.json.zip")
            if self.feed(channel) != value:
                raise ValueError("published feed read-back mismatch")


def prepare_promotion(
    candidate: Path,
    *,
    channel: str,
    repository: str,
    source_revision: str,
    timestamp: int,
) -> dict[str, Any]:
    if channel not in ("beta", "release"):
        raise ValueError("only beta/release promote existing alpha payloads")
    validated = dist.verify(candidate, channel="alpha")
    delivery_tests.require_evidence(candidate, validated)
    _identity(validated, repository, source_revision)
    for target in ("beta", "release") if channel == "release" else ("beta",):
        downloader_db.generate(
            candidate / "release-assets.json",
            candidate,
            target,
            repository,
            "v" + validated["version"],
            timestamp,
            asset_directory=candidate,
        )
    dist.write_checksums(candidate)
    return dist.verify(candidate, channel=channel)


def _identity(validated: dict[str, Any], repository: str, source_revision: str) -> None:
    if (
        not re.fullmatch(r"[0-9a-f]{40}", source_revision)
        or validated["source_revision"] != source_revision
        or validated["repository"] != repository
    ):
        raise ValueError(
            "promotion source/repository does not match validated candidate"
        )


def _verify_remote(github: GitHub, tag: str, candidate: Path, names: list[str]) -> None:
    with tempfile.TemporaryDirectory(prefix="magik-published-") as temporary:
        directory = Path(temporary)
        github.download(tag, directory, names)
        for name in names:
            if not (directory / name).is_file() or sha256_file(
                directory / name
            ) != sha256_file(candidate / name):
                raise ValueError(f"published asset differs: {tag}/{name}")


def publish(
    candidate: Path, *, channel: str, github: GitHub, source_revision: str
) -> dict[str, Any]:
    validated = dist.verify(candidate, channel=channel)
    delivery_tests.require_evidence(candidate, validated)
    _identity(validated, github.repository, source_revision)
    version = validated["version"]
    tag = "v" + version
    if channel != "alpha":
        prerequisite = "alpha" if channel == "beta" else "beta"
        expected = (candidate / f"mister-magik-{prerequisite}-db.json.zip").read_bytes()
        if github.feed(prerequisite) != expected:
            raise ValueError(
                f"promotion requires this exact candidate in the {prerequisite} feed"
            )
    release = github.release(tag)
    if channel == "alpha":
        names = sorted(path.name for path in candidate.iterdir())
        if not release:
            github.create(tag, source_revision)
            release = github.release(tag)
        if github.revision(tag) != source_revision:
            raise ValueError("immutable release tag points to another source revision")
        existing = {asset["name"] for asset in release["assets"]}
        if existing - set(names):
            raise ValueError("immutable release contains unexpected assets")
        # Reconcile uncertain/partial previous uploads; never clobber a payload.
        _verify_remote(github, tag, candidate, sorted(existing))
        github.upload(tag, [candidate / name for name in names if name not in existing])
    else:
        if not release or github.revision(tag) != source_revision:
            raise ValueError(
                "promoted immutable release is missing or has wrong source"
            )
        names = sorted({*validated["assets"], dist.RECEIPT, dist.EVIDENCE})
    _verify_remote(github, tag, candidate, names)
    targets = ("beta", "release") if channel == "release" else (channel,)
    feeds = {}
    for target in targets:
        if not github.release(target):
            github.create(target, source_revision)
        paths = [
            candidate / f"mister-magik-{target}-db.json",
            candidate / f"mister-magik-{target}-db.json.zip",
        ]
        if target != "alpha":
            paths.append(candidate / f"mister-magik-{target}-installer.zip")
        github.upload(target, paths, replace=True)
        _verify_remote(github, target, candidate, [path.name for path in paths])
        github.presentation(target, version, stable=channel == "release")
        feeds[f"mister-magik-{target}-db.json.zip"] = paths[1].read_bytes()
    if channel == "release":
        github.presentation(tag, version, stable=True)
    # This is the final visibility switch, and updates both stable feeds together.
    github.update_feed(feeds, version)
    return {
        "version": version,
        "channel": channel,
        "candidate_id": validated["candidate_id"],
        "published": True,
    }
