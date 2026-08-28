# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

from __future__ import annotations

import os
import subprocess
from pathlib import Path

COMMANDS = {
    "runtime-ci": ("apps/mister/Cargo.toml", "ci-fast", "ui"),
    "runtime-device": ("apps/mister/Cargo.toml", "release-device", "ui,profile"),
    "device-agent-ci": ("mister/tools/agent/Cargo.toml", "ci-fast", ""),
    "manager-device": ("mister/tools/manager/Cargo.toml", "release", ""),
}


def _environment(
    repository: Path, intent: str, profile: str, features: str, runner: str
) -> dict[str, str]:
    environment = {key: value for key, value in os.environ.items()}
    feature_set = set(features.split(","))
    if intent == "runtime-ci":
        environment.setdefault("MISTER_UI_BUILD_SCOPE", "all")
    elif intent == "runtime-device":
        environment.setdefault("MISTER_UI_BUILD_SCOPE", "production")
    if runner != "cross":
        return environment

    rustflags = "-D warnings -C target-cpu=cortex-a9"
    if "profile" in feature_set:
        rustflags += " -C force-frame-pointers=yes"
        environment["CFLAGS_armv7_unknown_linux_gnueabihf"] = (
            "-fno-omit-frame-pointer"
        )
    environment["RUSTFLAGS"] = rustflags

    if intent.startswith("runtime-"):
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


def execute(repository: Path, intent: str) -> None:
    if intent == "release-binaries":
        execute(repository, "runtime-device")
        execute(repository, "manager-device")
        return
    if intent not in COMMANDS:
        raise ValueError(f"unsupported CI build intent: {intent}")
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
        "armv7-unknown-linux-gnueabihf",
        "--profile",
        profile,
        "--locked",
    ]
    if features:
        command.extend(["--features", features.replace(",", ",")])
    environment = _environment(repository, intent, profile, features, runner)
    subprocess.run(command, cwd=repository, env=environment, check=True)
