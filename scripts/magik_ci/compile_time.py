"""Explicit build timing: two samples, no campaigns or automatic baseline changes."""

from __future__ import annotations

import json
import math
import os
import platform
import signal
import statistics
import subprocess
import time
from contextlib import ExitStack
from pathlib import Path

TARGETS = {
    "magik-full-app-macos": ("dev", "ui-preview", "launcher"),
    "magik-full-app-arm": ("release-live", "ui", "launcher"),
    "magik-release-device-arm-all": ("release-device", "ui,profile", "all"),
    "magik-release-device-arm-production": (
        "release-device",
        "ui,profile",
        "production",
    ),
    "magik-release-device-arm-thin": (
        "release-device-thin",
        "ui,profile",
        "production",
    ),
    "magik-release-device-arm-thin-stripped": (
        "release-device-thin-stripped",
        "ui,profile",
        "production",
    ),
    "magik-release-device-arm-thin-cgu32": (
        "release-device-thin-cgu32",
        "ui,profile",
        "production",
    ),
}


def command(root: Path, target: str, target_dir: Path, container: str | None = None):
    profile, features, scope = TARGETS[target]
    environment = dict(
        os.environ,
        RUSTC_WRAPPER="",
        SLINT_EMIT_DEBUG_INFO="1",
        MISTER_UI_BUILD_SCOPE=scope,
    )
    cargo = [
        "build",
        "--locked",
        "--manifest-path",
        "apps/mister/Cargo.toml",
        "--features",
        features,
    ]
    if target == "magik-full-app-macos":
        environment["CARGO_TARGET_DIR"] = str(target_dir)
        return [
            str(root / "scripts/cargo"),
            *cargo,
            "--bin",
            "mister-magik-ui-preview",
        ], environment
    if container is None:
        raise ValueError("ARM compilation requires an active build session")
    dist = "/workspace/apps/mister/target/ffmpeg-minimal/armv7/dist"
    values = {
        "CARGO_TARGET_DIR": "/workspace/" + str(target_dir.relative_to(root)),
        "MISTER_UI_BUILD_SCOPE": scope,
        "RUSTC_WRAPPER": "",
        "SLINT_EMIT_DEBUG_INFO": "1",
        "FFMPEG_DIR": dist,
        "PKG_CONFIG_PATH": dist + "/lib/pkgconfig",
        "PKG_CONFIG_ALLOW_CROSS": "1",
        "CFLAGS": "-I" + dist + "/include",
        "HOST_CFLAGS": "-I" + dist + "/include",
        "RUST_FONTCONFIG_DLOPEN": "1",
        "RUSTFLAGS": "-C target-cpu=cortex-a9"
        + (" -C force-frame-pointers=yes" if "profile" in features else ""),
    }
    env_args = [
        part for key, value in values.items() for part in ("--env", f"{key}={value}")
    ]
    return [
        "container",
        "exec",
        *env_args,
        "--workdir",
        "/workspace",
        container,
        "scripts/cargo",
        *cargo,
        "--profile",
        profile,
        "--bin",
        "mister-magik-fb",
        "--target",
        "armv7-unknown-linux-gnueabihf",
    ], environment


def measure(root: Path, target: str, target_dir: Path, output: Path, kind: str):
    root = root.resolve()
    target_dir = target_dir.resolve()
    target_dir.relative_to(
        root / "build"
    )  # Isolated local cache; never clean shared Cargo output.
    if output.exists():
        raise ValueError("refusing to overwrite build evidence")
    if kind == "cold" and target_dir.exists():
        raise ValueError("cold measurement requires a new target directory")
    if kind == "incremental" and not target_dir.is_dir():
        raise ValueError(
            "incremental measurements require an existing prepared target directory"
        )
    output.parent.mkdir(parents=True, exist_ok=True)
    report = {
        "schema": 1,
        "target": target,
        "kind": kind,
        "machine": platform.node(),
        "architecture": platform.machine(),
        "recipe": TARGETS[target],
        "flags": {
            name: os.environ.get(name, "")
            for name in ("RUSTFLAGS", "CARGO_ENCODED_RUSTFLAGS")
        },
        "dirty": bool(
            subprocess.check_output(
                ["git", "status", "--porcelain"], cwd=root, text=True
            ).strip()
        ),
        "samples": [],
        "revision": subprocess.check_output(
            ["git", "rev-parse", "HEAD"], cwd=root, text=True
        ).strip(),
    }
    try:
        with ExitStack() as session:
            container = None
            if target != "magik-full-app-macos":
                from magik2.host.magik2.ffmpeg import prepare_ffmpeg
                from magik2.host.magik2.storage import Storage

                storage = Storage()
                session.enter_context(storage.build_session(root))
                container = storage.prepare(root)
                prepare_ffmpeg(root, container, subprocess.run)
            for repetition in (1, 2):
                cache = target_dir / str(repetition) if kind == "cold" else target_dir
                argv, environment = command(root, target, cache, container)
                # Preparation is outside measured Cargo time. Logs go to files, never pipes.
                started = time.monotonic()
                with output.with_suffix(f".run-{repetition}.log").open("x") as log:
                    child = subprocess.Popen(
                        argv,
                        cwd=root,
                        env=environment,
                        stdout=log,
                        stderr=subprocess.STDOUT,
                        start_new_session=True,
                    )
                    try:
                        code = child.wait(timeout=1800)
                    finally:
                        # Include container/compilation descendants on interruption.
                        try:
                            os.killpg(child.pid, signal.SIGKILL)
                        except ProcessLookupError:
                            pass
                        child.wait(timeout=5)
                sample = {
                    "repetition": repetition,
                    "seconds": time.monotonic() - started,
                    "exit_code": code,
                }
                report["samples"].append(sample)
                if code:
                    raise RuntimeError(
                        f"compile sample {repetition} failed; see its log"
                    )
    except BaseException as error:
        report["error"] = str(error)
        raise
    finally:
        with output.open("x") as stream:
            json.dump(report, stream, indent=2)
            stream.write("\n")
    return report


def compare(baseline: dict, candidate: dict):
    for key in (
        "schema",
        "target",
        "kind",
        "machine",
        "architecture",
        "recipe",
        "flags",
    ):
        if baseline.get(key) != candidate.get(key):
            raise ValueError(f"incompatible build evidence: {key}")
    means = []
    for report in (baseline, candidate):
        samples = report.get("samples", [])
        if report.get("error") or [sample.get("repetition") for sample in samples] != [
            1,
            2,
        ]:
            raise ValueError("comparison requires two successful samples")
        values = [sample.get("seconds") for sample in samples]
        if any(sample.get("exit_code") != 0 for sample in samples) or any(
            type(value) not in (int, float) or not math.isfinite(value) or value <= 0
            for value in values
        ):
            raise ValueError("invalid compile timing sample")
        means.append(statistics.mean(values))
    return {
        "baseline_seconds": means[0],
        "candidate_seconds": means[1],
        "change_percent": 100 * (means[1] / means[0] - 1),
    }
