# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

from __future__ import annotations

import subprocess
from pathlib import Path
from unittest.mock import patch

import pytest

from scripts.magik_ci import build, ffmpeg


@pytest.fixture
def repository(tmp_path: Path) -> Path:
    app = tmp_path / "apps/mister"
    app.mkdir(parents=True)
    (app / "Cross.toml").write_text(
        '[target.armv7-unknown-linux-gnueabihf]\nimage = "test-image"\n'
    )
    return tmp_path


def outputs(repository: Path) -> None:
    for name in ffmpeg.REQUIRED:
        path = repository / ffmpeg.RELATIVE_WORK / "dist" / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text("fixture")


def test_cold_cache_builds_and_complete_cache_is_reused(repository: Path) -> None:
    with patch.object(ffmpeg.subprocess, "run") as run:
        run.side_effect = lambda *a, **kw: outputs(repository)
        ffmpeg.prepare(repository)
        assert [call.args[0][0] for call in run.call_args_list] == ["git", "docker"]
        docker = run.call_args_list[1].args[0]
        assert f"{repository}:/project" in docker
        assert docker[-1] == ffmpeg.RECIPE
        run.reset_mock()
        ffmpeg.prepare(repository)
        run.assert_not_called()


@pytest.mark.parametrize("damage", ["header", "archive", "stamp", "recipe"])
def test_incomplete_or_stale_cache_rebuilds(repository: Path, damage: str) -> None:
    with patch.object(ffmpeg.subprocess, "run") as run:
        run.side_effect = lambda *a, **kw: outputs(repository)
        ffmpeg.prepare(repository)
        dist = repository / ffmpeg.RELATIVE_WORK / "dist"
        if damage == "header":
            (dist / "include/libavutil/avutil.h").unlink()
        elif damage == "archive":
            (dist / "lib/libavcodec.a").unlink()
        elif damage == "stamp":
            (dist / ".magik-ci-ffmpeg-recipe").unlink()
        else:
            config = repository / "apps/mister/Cross.toml"
            config.write_text(config.read_text().replace("test-image", "new-image"))
        run.reset_mock()
        ffmpeg.prepare(repository)
        assert run.call_count == 2


def test_missing_outputs_never_stamp_success(repository: Path) -> None:
    with (
        patch.object(ffmpeg.subprocess, "run"),
        pytest.raises(RuntimeError, match="libavutil/avutil.h"),
    ):
        ffmpeg.prepare(repository)
    assert not (
        repository / ffmpeg.RELATIVE_WORK / "dist/.magik-ci-ffmpeg-recipe"
    ).exists()


@pytest.mark.parametrize("intent", ["runtime-ci", "runtime-device"])
def test_runtime_prepares_ffmpeg_before_cross(repository: Path, intent: str) -> None:
    events = []
    with (
        patch.dict("os.environ", {"MISTER_ARM_BUILD_BACKEND": "cross"}),
        patch.object(
            build.ffmpeg, "prepare", side_effect=lambda root: events.append("ffmpeg")
        ),
        patch.object(
            build.subprocess, "run", side_effect=lambda *a, **kw: events.append("cross")
        ),
        patch.object(build, "_write_build_identity"),
    ):
        build.execute(repository, intent)
    assert events == ["ffmpeg", "cross"]


def test_failed_preparation_stops_cargo(repository: Path) -> None:
    with (
        patch.dict("os.environ", {"MISTER_ARM_BUILD_BACKEND": "cross"}),
        patch.object(
            build.ffmpeg,
            "prepare",
            side_effect=subprocess.CalledProcessError(1, "ffmpeg"),
        ),
        patch.object(build.subprocess, "run") as run,
    ):
        with pytest.raises(subprocess.CalledProcessError):
            build.execute(repository, "runtime-ci")
        run.assert_not_called()


def test_cross_environment_uses_container_paths(repository: Path) -> None:
    env = build._environment(repository, "runtime-ci", "ci-fast", "ui", "cross")
    assert env["FFMPEG_DIR"] == str(ffmpeg.CONTAINER_DIST)
    assert env["PKG_CONFIG_PATH"] == str(ffmpeg.CONTAINER_DIST / "lib/pkgconfig")
    assert env["HOST_CFLAGS"] == f"-I{ffmpeg.CONTAINER_DIST}/include"
    assert env["CFLAGS"] == env["HOST_CFLAGS"]


def test_library_check_does_not_prepare_ffmpeg(repository: Path) -> None:
    with (
        patch.dict("os.environ", {"MISTER_ARM_BUILD_BACKEND": "cross"}),
        patch.object(build.ffmpeg, "prepare") as prepare,
        patch.object(build.subprocess, "run"),
    ):
        build.execute(repository, "runtime-library-ci")
        prepare.assert_not_called()
