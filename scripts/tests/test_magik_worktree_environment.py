# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

from __future__ import annotations

import os
import shutil
import stat
import subprocess
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]


def write_executable(path: Path, source: str) -> None:
    path.write_text(source)
    path.chmod(path.stat().st_mode | stat.S_IXUSR)


def run(command: list[str], *, cwd: Path) -> None:
    subprocess.run(command, cwd=cwd, check=True, capture_output=True)


def test_magik_check_reuses_only_a_compatible_primary_environment(
    tmp_path: Path,
) -> None:
    primary = tmp_path / "primary"
    linked = tmp_path / "linked"
    binary = tmp_path / "bin"
    capture = tmp_path / "uv-environment"

    (primary / "scripts/lib").mkdir(parents=True)
    (primary / "magik/host/.venv/bin").mkdir(parents=True)
    binary.mkdir()
    shutil.copy2(ROOT / "scripts/magik", primary / "scripts/magik")
    shutil.copy2(
        ROOT / "scripts/lib/shared-worktree-cache.sh",
        primary / "scripts/lib/shared-worktree-cache.sh",
    )
    (primary / "magik/host/pyproject.toml").write_text("[project]\nname='fixture'\n")
    (primary / "magik/host/uv.lock").write_text("version = 1\n")
    write_executable(primary / "magik/host/.venv/bin/python", "#!/bin/sh\nexit 0\n")
    write_executable(
        binary / "uv",
        "#!/bin/sh\n"
        'printf "%s\\n" "${UV_PROJECT_ENVIRONMENT:-unset}" >"$UV_CAPTURE"\n'
        'printf "%s\\n" "$*" >>"$UV_CAPTURE"\n',
    )

    run(["git", "init", "-q"], cwd=primary)
    run(["git", "config", "user.email", "test@example.invalid"], cwd=primary)
    run(["git", "config", "user.name", "Test"], cwd=primary)
    run(["git", "add", "."], cwd=primary)
    run(["git", "commit", "-qm", "fixture"], cwd=primary)
    run(["git", "branch", "-M", "main"], cwd=primary)
    run(
        ["git", "worktree", "add", "-q", "-b", "feature", str(linked), "main"],
        cwd=primary,
    )

    environment = os.environ.copy()
    environment.update(
        {
            "PATH": f"{binary}{os.pathsep}{environment['PATH']}",
            "UV_CAPTURE": str(capture),
        }
    )
    subprocess.run(
        [str(linked / "scripts/magik"), "check", "--help"],
        cwd=linked,
        env=environment,
        check=True,
    )
    lines = capture.read_text().splitlines()
    assert lines[0] == str(primary / "magik/host/.venv")
    assert "--extra testing" in lines[1]

    (linked / "magik/host/uv.lock").write_text("version = 2\n")
    subprocess.run(
        [str(linked / "scripts/magik"), "check", "--help"],
        cwd=linked,
        env=environment,
        check=True,
    )
    assert capture.read_text().splitlines()[0] == "unset"

    environment["UV_PROJECT_ENVIRONMENT"] = str(tmp_path / "explicit-environment")
    subprocess.run(
        [str(linked / "scripts/magik"), "check", "--help"],
        cwd=linked,
        env=environment,
        check=True,
    )
    assert capture.read_text().splitlines()[0] == environment["UV_PROJECT_ENVIRONMENT"]
