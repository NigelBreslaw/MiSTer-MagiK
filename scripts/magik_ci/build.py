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
    environment = os.environ.copy()
    if intent == "runtime-ci":
        environment.setdefault("MISTER_UI_BUILD_SCOPE", "all")
    elif intent == "runtime-device":
        environment.setdefault("MISTER_UI_BUILD_SCOPE", "production")
    subprocess.run(command, cwd=repository, env=environment, check=True)
