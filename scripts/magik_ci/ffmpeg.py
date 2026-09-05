# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

"""Prepare the minimal ARM FFmpeg dependency before invoking cross."""

from __future__ import annotations

import hashlib
import os
import shlex
import shutil
import subprocess
import tomllib
from pathlib import Path

VERSION = "8.1.2"
RELATIVE_WORK = Path("apps/mister/target/ffmpeg-minimal/armv7")
CONTAINER_DIST = Path("/project") / RELATIVE_WORK / "dist"
LIBRARIES = ("avcodec", "avformat", "avutil", "swresample")
REQUIRED = tuple(
    name
    for library in LIBRARIES
    for name in (
        f"include/lib{library}/{library}.h",
        f"lib/lib{library}.a",
        f"lib/pkgconfig/lib{library}.pc",
    )
) + ("include/libavcodec/version_major.h",)
CONFIGURE = (
    f"--prefix={CONTAINER_DIST}",
    "--cross-prefix=arm-linux-gnueabihf-",
    "--arch=arm",
    "--cpu=cortex-a9",
    "--target-os=linux",
    "--enable-cross-compile",
    "--extra-cflags=-O3 -mcpu=cortex-a9 -mfpu=neon-vfpv3 -mfloat-abi=hard",
    "--extra-cxxflags=-O3 -mcpu=cortex-a9 -mfpu=neon-vfpv3 -mfloat-abi=hard",
    "--enable-static",
    "--disable-shared",
    "--enable-pic",
    "--disable-autodetect",
    "--disable-programs",
    "--disable-doc",
    "--disable-debug",
    "--enable-stripping",
    "--disable-everything",
    "--disable-avdevice",
    "--disable-avfilter",
    "--enable-swresample",
    "--enable-avcodec",
    "--enable-avformat",
    "--enable-avutil",
    "--disable-swscale",
    "--enable-decoder=h264",
    "--enable-decoder=aac",
    "--enable-decoder=pcm_s16le",
    "--enable-parser=aac",
    "--enable-parser=h264",
    "--enable-demuxer=mov",
    "--enable-protocol=file",
)
RECIPE = (
    shlex.join(("./configure", *CONFIGURE))
    + "\n"
    + "\n".join(
        f"grep -q '^#define CONFIG_{flag} 0$' config.h"
        for flag in ("GPL", "VERSION3", "NONFREE")
    )
    + "\nmake install"
)


def prepare(repository: Path) -> None:
    config = (repository / "apps/mister/Cross.toml").read_text()
    image = tomllib.loads(config)["target"]["armv7-unknown-linux-gnueabihf"]["image"]
    identity = hashlib.sha256(
        f"{VERSION}\n{config}\n{RECIPE}\n{REQUIRED}".encode()
    ).hexdigest()
    work = repository / RELATIVE_WORK
    dist = work / "dist"
    stamp = dist / ".magik-ci-ffmpeg-recipe"
    if (
        stamp.is_file()
        and stamp.read_text().strip() == identity
        and all((dist / name).is_file() for name in REQUIRED)
    ):
        return
    work.mkdir(parents=True, exist_ok=True)
    # Only generated dependency outputs are replaced, never repository sources.
    if dist.exists():
        shutil.rmtree(dist)
    source = work / f"ffmpeg-{VERSION}"
    if source.exists():
        shutil.rmtree(source)
    subprocess.run(
        [
            "git",
            "clone",
            "--depth=1",
            "--branch",
            f"n{VERSION}",
            "https://github.com/FFmpeg/FFmpeg",
            str(source),
        ],
        check=True,
    )
    subprocess.run(
        [
            "docker",
            "run",
            "--rm",
            "--platform",
            "linux/amd64",
            "--user",
            f"{os.getuid()}:{os.getgid()}",
            "-e",
            f"MAKEFLAGS=-j{os.cpu_count() or 1}",
            "-v",
            f"{repository}:/project",
            "-w",
            str(Path("/project") / source.relative_to(repository)),
            image,
            "sh",
            "-ec",
            RECIPE,
        ],
        cwd=repository,
        check=True,
    )
    missing = [name for name in REQUIRED if not (dist / name).is_file()]
    if missing:
        raise RuntimeError("minimal FFmpeg outputs missing: " + ", ".join(missing))
    stamp.write_text(identity + "\n")
