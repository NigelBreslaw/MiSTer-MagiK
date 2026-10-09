# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

import hashlib
import importlib.util
import os
import runpy
import subprocess
from pathlib import Path
from unittest.mock import patch

import pytest

ROOT = Path(__file__).resolve().parents[2]
SPEC = importlib.util.spec_from_file_location(
    "third_party_licenses",
    ROOT / "scripts/release/packaging/generate-third-party-licenses.py",
)
assert SPEC is not None and SPEC.loader is not None
licenses = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(licenses)
BUILDERS = ("magik/host/magik/ffmpeg.py", "scripts/magik_ci/ffmpeg.py")


def test_current_build_recipes_generate_notices(tmp_path, monkeypatch):
    monkeypatch.setattr(
        licenses,
        "metadata",
        lambda: {
            "resolve": {"root": "app", "nodes": [{"id": "app", "deps": []}]},
            "workspace_members": ["app"],
            "packages": [],
        },
    )
    monkeypatch.setattr(licenses, "OUTPUT", tmp_path / "RUST-LIBRARIES.txt")
    monkeypatch.setattr(licenses, "FONT_OUTPUT", tmp_path / "PRESS-START-2P.txt")
    # main's status line uses repository-relative output paths.
    monkeypatch.setattr(licenses, "ROOT", tmp_path)
    licenses.main()
    assert (tmp_path / "RUST-LIBRARIES.txt").read_text().startswith("RUST LIBRARIES")
    assert (
        tmp_path / "PRESS-START-2P.txt"
    ).read_text() == licenses.FONT_LICENSE.read_text()


@pytest.mark.parametrize("builder", BUILDERS)
@pytest.mark.parametrize(
    "unsafe",
    [
        "--disable-autodetect",
        "--disable-everything",
        "--disable-shared",
        "CONFIG_GPL 0",
        "CONFIG_VERSION3 0",
        "CONFIG_NONFREE 0",
        "--enable-gpl",
        "--enable-version3",
        "--enable-nonfree",
    ],
)
def test_unsafe_executed_recipe_is_rejected(builder, unsafe, monkeypatch):
    load = runpy.run_path

    def altered(path):
        data = load(path)
        if str(path).endswith(builder):
            recipe = data["RECIPE"]
            data["RECIPE"] = (
                recipe + " " + unsafe
                if unsafe.startswith("--enable-")
                else recipe.replace(unsafe, "removed")
            )
        return data

    monkeypatch.setattr(licenses.runpy, "run_path", altered)
    with pytest.raises(SystemExit, match="FFmpeg license gate"):
        licenses.validate_ffmpeg_recipes()


def test_host_builder_executes_checks_and_invalidates_old_cache(tmp_path):
    host = runpy.run_path(ROOT / BUILDERS[0])
    work = tmp_path / "apps/mister/target/ffmpeg-minimal/armv7"
    stamp = work / "dist/.magik-recipe"
    stamp.parent.mkdir(parents=True)
    stamp.write_text(hashlib.sha256(host["CONFIGURE"].encode()).hexdigest())
    for library in ("avcodec", "avformat", "avutil", "swresample"):
        archive = work / f"dist/lib/lib{library}.a"
        archive.parent.mkdir(parents=True, exist_ok=True)
        archive.write_bytes(b"fixture")
    (work / "ffmpeg-8.1.2/.git").mkdir(parents=True)
    with patch("subprocess.run") as runner:
        host["prepare_ffmpeg"](tmp_path, "test-container", runner)
        recipe = runner.call_args.args[0][-1]
        for flag in ("GPL", "VERSION3", "NONFREE"):
            assert f"grep -q '^#define CONFIG_{flag} 0$' config.h" in recipe
        runner.reset_mock()
        host["prepare_ffmpeg"](tmp_path, "test-container", runner)
        runner.assert_not_called()


def test_inventory_uses_the_shipped_arm_features():
    with patch.object(
        licenses.subprocess, "check_output", return_value=b"{}"
    ) as metadata:
        licenses.metadata()
    command = metadata.call_args.args[0]
    assert command[command.index("--features") + 1] == "ui"
    assert (
        command[command.index("--filter-platform") + 1]
        == "armv7-unknown-linux-gnueabihf"
    )


@pytest.mark.parametrize("builder", BUILDERS)
@pytest.mark.parametrize("flag", ["GPL", "VERSION3", "NONFREE"])
def test_generated_config_rejects_unsafe_ffmpeg_before_install(tmp_path, builder, flag):
    (tmp_path / "configure").write_text("#!/bin/sh\nexit 0\n")
    (tmp_path / "configure").chmod(0o755)
    (tmp_path / "make").write_text("#!/bin/sh\ntouch installed\n")
    (tmp_path / "make").chmod(0o755)
    (tmp_path / "config.h").write_text(
        "".join(
            f"#define CONFIG_{name} {int(name == flag)}\n"
            for name in ("GPL", "VERSION3", "NONFREE")
        )
    )
    recipe = runpy.run_path(ROOT / builder)["RECIPE"]
    result = subprocess.run(
        ["sh", "-ec", recipe],
        cwd=tmp_path,
        env={**os.environ, "PATH": str(tmp_path) + os.pathsep + os.environ["PATH"]},
        capture_output=True,
        check=False,
    )
    assert result.returncode != 0
    assert not (tmp_path / "installed").exists()
