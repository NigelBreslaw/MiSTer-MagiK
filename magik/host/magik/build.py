"""Per-checkout ARM builds with Cargo-owned dependency tracking."""

from __future__ import annotations

import json
import os
import subprocess
import sys
import time
import tempfile
from dataclasses import dataclass
from pathlib import Path
from collections.abc import Callable

TARGET = "armv7-unknown-linux-gnueabihf"
RUST_TOOLCHAIN = "1.99.0"


@dataclass(frozen=True)
class BuildResult:
    artifact: Path
    rebuilt: bool
    elapsed_ms: int
    prebuilt: bool = False
    source_revision: str | None = None
    source_dirty: bool | None = None


def source_metadata(repository: Path) -> tuple[str | None, bool | None]:
    try:
        revision = subprocess.check_output(
            ["git", "rev-parse", "HEAD"], cwd=repository, text=True
        ).strip()
        dirty = bool(
            subprocess.check_output(
                ["git", "status", "--porcelain", "--untracked-files=all"],
                cwd=repository,
                text=True,
            ).strip()
        )
        return revision, dirty
    except (OSError, subprocess.CalledProcessError):
        return None, None


def build_repository(package: Path) -> Path:
    return package.resolve().parents[2 if package.name == "manager" else 1]


def ensure_arm_package(
    package: Path,
    *,
    runner: Callable = subprocess.run,
    prepare: Callable | None = None,
) -> BuildResult:
    # Injectable preparation keeps pure build tests independent of Apple Container.
    if prepare is not None:
        return _ensure_arm_package(package, runner=runner, prepare=prepare)
    from .storage import Storage

    storage = Storage(runner=runner)
    with storage.build_session(build_repository(package)):
        return _ensure_arm_package(
            package,
            runner=runner,
            prepare=lambda repository, _: storage.prepare(repository),
        )


def _ensure_arm_package(
    package: Path,
    *,
    runner: Callable = subprocess.run,
    prepare: Callable | None = None,
) -> BuildResult:
    from .apps import APPLICATIONS

    app = next(
        (
            value
            for value in APPLICATIONS.values()
            if value.package.endswith("/" + package.name)
        ),
        None,
    )
    if app:
        from .apps import application

        app = application(app.name)
    profile = app.profile if app else "release"
    binary = (
        app.binary
        if app
        else "mister-magik-manager"
        if package.name == "manager"
        else "mister-magik-service"
    )
    artifact = package / "target" / TARGET / profile / binary
    repository = build_repository(package)
    if (
        app
        and app.name == "magik"
        and not (repository / "private/magik-assets/.git").exists()
    ):
        runner(
            [
                "git",
                "-C",
                str(repository),
                "submodule",
                "update",
                "--init",
                "--checkout",
                "--",
                "private/magik-assets",
            ],
            check=True,
        )
    started = time.monotonic()
    from .preflight import require_space

    require_space(repository, 8 * 1024**3, "ARM build")
    name = prepare(repository, runner)
    environment = []
    revision, dirty = (
        source_metadata(repository) if app and app.name == "magik" else (None, None)
    )
    if revision is not None:
        environment += ["--env", f"MISTER_MAGIK_SOURCE_REVISION={revision}"]
    if dirty is not None:
        environment += ["--env", f"MISTER_MAGIK_SOURCE_DIRTY={int(dirty)}"]
    if app and app.name == "mini-magik":
        environment += ["--env", f"MAGIK_MINI_BUILD_PROFILE={profile}"]
        if profile == "release-device":
            environment += [
                "--env",
                "RUSTFLAGS=-C target-cpu=cortex-a9 -C force-frame-pointers=yes",
            ]
    if app and app.name == "magik":
        from .ffmpeg import prepare_ffmpeg

        prepare_ffmpeg(repository, name, runner)
        dist = "/workspace/apps/mister/target/ffmpeg-minimal/armv7/dist"
        for key, value in {
            "FFMPEG_DIR": dist,
            "PKG_CONFIG_PATH": dist + "/lib/pkgconfig",
            "PKG_CONFIG_ALLOW_CROSS": "1",
            "CFLAGS": "-I" + dist + "/include",
            "HOST_CFLAGS": "-I" + dist + "/include",
            "RUST_FONTCONFIG_DLOPEN": "1",
            "SLINT_EMIT_DEBUG_INFO": "1",
            "RUSTFLAGS": "-C target-cpu=cortex-a9 -C force-frame-pointers=yes",
        }.items():
            environment += ["--env", f"{key}={value}"]
    for variable in ("RUSTFLAGS", "CARGO_ENCODED_RUSTFLAGS"):
        if variable in os.environ:
            environment += ["--env", f"{variable}={os.environ[variable]}"]
    # Spool Cargo messages to keep memory bounded; Cargo progress stays on stderr.
    with tempfile.TemporaryFile(mode="w+") as messages:
        result = runner(
            [
                "container",
                "exec",
                *environment,
                "--workdir",
                f"/workspace/{package.resolve().relative_to(repository)}",
                name,
                "cargo",
                "build",
                "--locked",
                "--message-format=json-render-diagnostics",
                "--profile",
                profile,
                *(
                    ["--features", ",".join(app.features)]
                    if app and app.features
                    else []
                ),
                "--bin",
                binary,
                "--target",
                TARGET,
            ],
            check=False,
            stdout=messages,
        )
        messages.seek(0)
        fresh = None
        expected_executable = str(
            Path("/workspace") / artifact.resolve().relative_to(repository)
        )
        for line in messages:
            try:
                message = json.loads(line)
            except ValueError:
                print(line, end="", file=sys.stderr)
                continue
            if message.get("reason") == "compiler-message":
                rendered = message.get("message", {}).get("rendered")
                if rendered:
                    print(rendered, end="", file=sys.stderr)
            elif (
                message.get("reason") == "compiler-artifact"
                and message.get("target", {}).get("name") == binary
                and "bin" in message.get("target", {}).get("kind", [])
                and message.get("executable") == expected_executable
                and type(message.get("fresh")) is bool
            ):
                fresh = message["fresh"]
    if result.returncode or fresh is None or not artifact.is_file():
        raise RuntimeError(
            f"MagiK ARM {package.name} build failed or omitted its binary artifact"
        )
    return BuildResult(
        artifact,
        not fresh,
        int((time.monotonic() - started) * 1000),
        source_revision=revision,
        source_dirty=dirty,
    )


def ensure_arm_application(
    probe_root: Path,
    *,
    runner: Callable = subprocess.run,
    prepare: Callable | None = None,
) -> BuildResult:
    prebuilt = os.environ.get("MISTER_MAGIK2_PREBUILT_ARTIFACT")
    if prebuilt:
        # Validate selectors even though a prebuilt binary cannot be reconfigured.
        # The scenario session verifies the running binary's Analytics context.
        if probe_root.name == "mister":
            from .apps import application

            application("magik")
        artifact = Path(prebuilt).expanduser().resolve()
        if not artifact.is_file():
            raise RuntimeError("MagiK prebuilt application artifact is unavailable")
        dirty = os.environ.get("MISTER_MAGIK2_PREBUILT_SOURCE_DIRTY")
        if dirty not in {None, "0", "1"}:
            raise ValueError("prebuilt source dirty must be 0 or 1")
        return BuildResult(
            artifact,
            False,
            0,
            prebuilt=True,
            source_revision=os.environ.get("MISTER_MAGIK2_PREBUILT_SOURCE_REVISION"),
            source_dirty=None if dirty is None else dirty == "1",
        )
    return ensure_arm_package(probe_root, runner=runner, prepare=prepare)


def ensure_arm_agent() -> Path:
    package = Path(__file__).resolve().parents[2] / "agent"
    return ensure_arm_package(package).artifact
