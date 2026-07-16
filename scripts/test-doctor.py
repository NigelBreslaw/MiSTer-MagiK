#!/usr/bin/env python3
# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

from __future__ import annotations

import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile


ROOT = Path(__file__).resolve().parent.parent
DOCTOR = ROOT / "scripts/doctor.py"


def write_command(bin_dir: Path, name: str, body: str = "exit 0") -> None:
    path = bin_dir / name
    path.write_text(f"#!/bin/sh\n{body}\n", encoding="utf-8")
    path.chmod(0o755)


def fixture(root: Path, bin_dir: Path) -> None:
    (root / "magik-gui").mkdir(parents=True)
    (root / "magik-gui/rust-toolchain.toml").write_text(
        '[toolchain]\nchannel = "1.97.0"\n', encoding="utf-8"
    )
    for submodule in ("github-app", "material-icon-theme"):
        path = root / "desktop/vendor" / submodule
        path.mkdir(parents=True)
        (path / ".git").write_text("gitdir: fixture\n", encoding="utf-8")
    modules = root / "documentation/node_modules"
    (modules / ".pnpm").mkdir(parents=True)
    (modules / ".bin").mkdir()
    write_command(modules / ".bin", "astro")
    (root / "scripts").mkdir()
    write_command(root / "scripts", "validate")
    write_command(root / "scripts", "dev-rust")
    write_command(root / "scripts", "mister")
    write_command(root / "scripts", "test-host-tools.sh")
    (root / ".gitignore").write_text(
        "/build/\n/dist/\n/outputs/\n/target/\n/documentation/dist/\n",
        encoding="utf-8",
    )
    for command in ("python3", "git", "cargo"):
        write_command(bin_dir, command)
    write_command(
        bin_dir,
        "rustup",
        """case "$1 $2" in
"toolchain list") printf '1.97.0-aarch64-apple-darwin\\n' ;;
"component list") printf 'clippy-aarch64-apple-darwin (installed)\\nrustfmt-aarch64-apple-darwin (installed)\\n' ;;
"target list") printf 'armv7-unknown-linux-gnueabihf (installed)\\n' ;;
*) exit 2 ;;
esac""",
    )
    write_command(
        bin_dir,
        "git",
        """if [ "$3" = check-ignore ]; then exit 0; fi
exit 0""",
    )
    write_command(bin_dir, "node", "printf 'v24.0.0\\n'")
    write_command(
        bin_dir,
        "corepack",
        "if [ \"$1\" = pnpm ]; then printf '11.10.0\\n'; else exit 2; fi",
    )


def run(root: Path, bin_dir: Path, scope: str) -> subprocess.CompletedProcess[str]:
    env = os.environ.copy()
    env["PATH"] = f"{bin_dir}:/usr/bin:/bin"
    return subprocess.run(
        [sys.executable, str(DOCTOR), "--root", str(root), "--scope", scope, "--json"],
        check=False,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
        env=env,
    )


def main() -> int:
    with tempfile.TemporaryDirectory() as raw:
        base = Path(raw)
        root = base / "repo"
        bin_dir = base / "bin"
        root.mkdir()
        bin_dir.mkdir()
        fixture(root, bin_dir)

        result = run(root, bin_dir, "full-host")
        assert result.returncode == 0, result.stderr
        value = json.loads(result.stdout)
        assert value["schema"] == "mister-magik-doctor-v1"
        assert value["ok"] is True
        assert value["device_probe"] == "not_attempted"

        (bin_dir / "cargo").unlink()
        result = run(root, bin_dir, "full-host")
        value = json.loads(result.stdout)
        assert result.returncode == 1
        assert value["ok"] is False
        assert any(check["id"] == "command-cargo" and check["status"] == "fail" for check in value["checks"])

        result = run(root, bin_dir, "device")
        value = json.loads(result.stdout)
        assert value["requires_device"] is True
        assert value["device_probe"] == "not_attempted"
        assert "password" not in result.stdout.lower()
        network = next(check for check in value["checks"] if check["id"] == "device-network")
        assert network["status"] == "warn"

        write_command(bin_dir, "git", "exit 1")
        result = run(root, bin_dir, "full-host")
        value = json.loads(result.stdout)
        assert result.returncode == 1
        assert any(
            check["id"].startswith("output-ignored-") and check["status"] == "fail"
            for check in value["checks"]
        )

    print("doctor tests ok")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
