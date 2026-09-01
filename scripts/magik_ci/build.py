# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

from __future__ import annotations

import hashlib
import os
import subprocess
from pathlib import Path

TARGET = "armv7-unknown-linux-gnueabihf"
COMMANDS = {
    "runtime-ci": ("apps/mister/Cargo.toml", "ci-fast", "ui"),
    "runtime-device": ("apps/mister/Cargo.toml", "release-device", "ui,profile"),
    "device-agent-ci": ("mister/tools/agent/Cargo.toml", "ci-fast", ""),
    "manager-device": ("mister/tools/manager/Cargo.toml", "release", ""),
}
CHECKS = {
    "runtime-library-ci": ("apps/mister/Cargo.toml", "all", ""),
}
ARTIFACTS = {
    "runtime-ci": "apps/mister/target/armv7-unknown-linux-gnueabihf/ci-fast/mister-magik-fb",
    "runtime-device": "apps/mister/target/armv7-unknown-linux-gnueabihf/release-device/mister-magik-fb",
    "device-agent-ci": "mister/tools/agent/target/armv7-unknown-linux-gnueabihf/ci-fast/mister-magik-agent",
    "manager-device": "mister/tools/manager/target/armv7-unknown-linux-gnueabihf/release/mister-magik-manager",
}


def _environment(
    repository: Path, intent: str, profile: str, features: str, runner: str
) -> dict[str, str]:
    environment = {key: value for key, value in os.environ.items()}
    feature_set = set(features.split(","))
    if intent in {"runtime-ci", "runtime-library-ci"}:
        environment.setdefault("MISTER_UI_BUILD_SCOPE", "all")
    elif intent == "runtime-device":
        environment.setdefault("MISTER_UI_BUILD_SCOPE", "production")
    if runner != "cross":
        return environment

    rustflags = "-D warnings -C target-cpu=cortex-a9"
    if "profile" in feature_set:
        rustflags += " -C force-frame-pointers=yes"
        environment["CFLAGS_armv7_unknown_linux_gnueabihf"] = "-fno-omit-frame-pointer"
    environment["RUSTFLAGS"] = rustflags

    if runner == "cross" and intent in {"runtime-ci", "runtime-device"}:
        dist = repository / "apps/mister/target/ffmpeg-minimal/armv7/dist"
        include = dist / "include"
        environment.update(
            {
                "FFMPEG_DIR": str(dist),
                "PKG_CONFIG_PATH": str(dist / "lib/pkgconfig"),
                "PKG_CONFIG_ALLOW_CROSS": "1",
                "CFLAGS": f"-I{include}",
                "HOST_CFLAGS": f"-I{include}",
            }
        )
    return environment


def _sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def _git(repository: Path, *arguments: str) -> str:
    return subprocess.check_output(
        ["git", "-C", str(repository), *arguments], text=True
    ).strip()


def _write_build_identity(
    repository: Path,
    intent: str,
    profile: str,
    features: str,
    runner: str,
) -> None:
    if intent not in ARTIFACTS:
        return
    artifact = repository / ARTIFACTS[intent]
    if not artifact.is_file():
        raise RuntimeError(f"build completed without its expected output: {artifact}")

    build_number = (
        os.environ.get("MISTER_MAGIK_BUILD_NUMBER")
        or os.environ.get("RELEASE_BUILD_NUMBER")
        or _git(repository, "rev-list", "--count", "HEAD")
    )
    version = (
        os.environ.get("MISTER_MAGIK_VERSION")
        or os.environ.get("RELEASE_VERSION")
        or f"0.2.{build_number}"
    )
    source_revision = os.environ.get("MISTER_MAGIK_SOURCE_REVISION") or _git(
        repository, "rev-parse", "HEAD"
    )
    source_dirty = os.environ.get("MISTER_MAGIK_SOURCE_DIRTY", "0")
    ui_scope = "production" if intent == "runtime-device" else "all"
    lockfile = repository / Path(COMMANDS[intent][0]).with_name("Cargo.lock")
    toolchain = repository / "apps/mister/rust-toolchain.toml"
    cache_identity = f"v-python:{intent}:{profile}:{features}:{runner}"

    artifact.with_name(f"{artifact.name}.features").write_text(
        features, encoding="utf-8"
    )
    receipt = (
        "build_receipt_tsv"
        f"\tbinary_sha256={_sha256(artifact)}"
        f"\tprofile={profile}"
        f"\tfeatures={features}"
        f"\tui_scope={ui_scope}"
        f"\tbuild_number={build_number}"
        f"\tversion={version}"
        f"\tsource_commit={source_revision}"
        f"\tsource_dirty={source_dirty}"
        f"\tcache_identity={cache_identity}"
        f"\tlock_sha256={_sha256(lockfile)}"
        f"\ttoolchain_sha256={_sha256(toolchain)}\n"
    )
    receipt_path = Path(f"{artifact}.build-receipt.tsv")
    receipt_tmp = receipt_path.with_name(f"{receipt_path.name}.tmp")
    receipt_tmp.write_text(receipt, encoding="utf-8")
    receipt_tmp.replace(receipt_path)


def execute(repository: Path, intent: str) -> None:
    if intent == "release-binaries":
        execute(repository, "runtime-device")
        execute(repository, "manager-device")
        return
    if intent not in COMMANDS:
        if intent not in CHECKS:
            raise ValueError(f"unsupported CI build intent: {intent}")
        manifest, profile, features = CHECKS[intent]
        runner = (
            "cross"
            if os.environ.get("MISTER_ARM_BUILD_BACKEND") == "cross"
            else "cargo"
        )
        command = [
            runner,
            "check",
            "--manifest-path",
            str(repository / manifest),
            "--target",
            TARGET,
            "--locked",
            "--lib",
            "--no-default-features",
        ]
        environment = _environment(repository, intent, profile, features, runner)
        subprocess.run(command, cwd=repository, env=environment, check=True)
        return
    manifest, profile, features = COMMANDS[intent]
    runner = (
        "cross" if os.environ.get("MISTER_ARM_BUILD_BACKEND") == "cross" else "cargo"
    )
    command = [
        runner,
        "build",
        "--manifest-path",
        str(repository / manifest),
        "--target",
        TARGET,
        "--profile",
        profile,
        "--locked",
    ]
    if features:
        command.extend(["--features", features.replace(",", ",")])
    if os.environ.get("MISTER_CARGO_TIMINGS") == "1":
        command.append("--timings")
    environment = _environment(repository, intent, profile, features, runner)
    subprocess.run(command, cwd=repository, env=environment, check=True)
    _write_build_identity(repository, intent, profile, features, runner)
