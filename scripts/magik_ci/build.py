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
    command = [
        "cargo",
        "build",
        "--manifest-path",
        str(repository / manifest),
        "--profile",
        profile,
        "--locked",
    ]
    if features:
        command.extend(["--features", features.replace(",", ",")])
    environment = os.environ.copy()
    if intent.endswith("ci"):
        environment.setdefault("MISTER_UI_BUILD_SCOPE", "all")
    subprocess.run(command, cwd=repository, env=environment, check=True)
