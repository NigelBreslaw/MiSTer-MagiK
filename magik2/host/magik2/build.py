"""Reproducible per-checkout ARM builds with a small, content-validated cache."""

from __future__ import annotations

import hashlib
import json
import os
import re
import subprocess
import time
import tomllib
import tempfile
from dataclasses import dataclass
from pathlib import Path
from collections.abc import Callable

TARGET = "armv7-unknown-linux-gnueabihf"
RUST_TOOLCHAIN = "1.98.0"
EXCLUDED = {"target", ".git", ".venv", "__pycache__", "build", "outputs"}


@dataclass(frozen=True)
class BuildResult:
    artifact: Path
    rebuilt: bool
    elapsed_ms: int
    prebuilt: bool = False
    fingerprint: str = ""


def relevant_inputs(package: Path) -> list[Path]:
    """Walk local Cargo dependencies, never generated outputs or unrelated crates."""
    inputs: set[Path] = set()
    visited: set[Path] = set()

    def visit(root: Path) -> None:
        root = root.resolve()
        if root in visited:
            return
        visited.add(root)
        for directory, folders, files in os.walk(root):
            folders[:] = [name for name in folders if name not in EXCLUDED]
            for name in files:
                path = Path(directory) / name
                if path.suffix in {
                    ".rs",
                    ".slint",
                    ".ttf",
                    ".otf",
                    ".png",
                    ".svg",
                } or name in {
                    "Cargo.toml",
                    "Cargo.lock",
                    "config.toml",
                    "rust-toolchain.toml",
                }:
                    inputs.add(path)
        manifest = root / "Cargo.toml"
        if manifest.exists():

            def dependencies(value: object) -> None:
                if isinstance(value, dict):
                    for key, child in value.items():
                        if key in {"dependencies", "build-dependencies"} and isinstance(
                            child, dict
                        ):
                            for dependency in child.values():
                                if (
                                    isinstance(dependency, dict)
                                    and "path" in dependency
                                ):
                                    visit(root / dependency["path"])
                        elif key == "target":
                            for target in child.values():
                                dependencies(target)

            dependencies(tomllib.loads(manifest.read_text()))

    visit(package)
    # Slint embeds fonts/images outside a package. Follow quoted file imports.
    pending = list(inputs)
    while pending:
        path = pending.pop()
        if path.suffix != ".slint":
            continue
        for imported in re.findall(
            r'["\']([^"\']+\.(?:slint|ttf|otf|png|svg))["\']', path.read_text()
        ):
            candidate = (path.parent / imported).resolve()
            if candidate.is_file() and candidate not in inputs:
                inputs.add(candidate)
                pending.append(candidate)
    return sorted(inputs)


def source_fingerprint(package: Path) -> str:
    digest = hashlib.sha256()
    for path in relevant_inputs(package):
        digest.update(
            os.path.relpath(path, package).encode() + b"\0" + path.read_bytes() + b"\0"
        )
    recipe = Path(__file__).resolve().parents[2] / "build/Containerfile"
    digest.update(recipe.read_bytes())
    digest.update(Path(__file__).read_bytes())
    digest.update(
        json.dumps(
            {
                name: os.environ.get(name, "")
                for name in ("RUSTFLAGS", "CARGO_ENCODED_RUSTFLAGS")
            },
            sort_keys=True,
        ).encode()
    )
    return digest.hexdigest()


def artifact_hash(artifact: Path) -> str:
    return hashlib.sha256(artifact.read_bytes()).hexdigest()


def needs_build(cache_file: Path, fingerprint: str) -> bool:
    try:
        cached = json.loads(cache_file.read_text())
        return (
            cached["fingerprint"] != fingerprint
            or artifact_hash(Path(cached["artifact"])) != cached["sha256"]
        )
    except (OSError, ValueError, KeyError):
        return True


def write_build_cache(cache_file: Path, fingerprint: str, artifact: Path) -> None:
    cache_file.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.NamedTemporaryFile(
        mode="w", dir=cache_file.parent, delete=False
    ) as output:
        temporary = Path(output.name)
        json.dump(
            {
                "fingerprint": fingerprint,
                "artifact": str(artifact.resolve()),
                "sha256": artifact_hash(artifact),
            },
            output,
        )
    try:
        temporary.replace(cache_file)
    finally:
        temporary.unlink(missing_ok=True)


def prepare_container(repository: Path, runner: Callable = subprocess.run) -> str:
    recipe = repository / "magik2/build/Containerfile"
    recipe_id = hashlib.sha256(recipe.read_bytes()).hexdigest()[:12]
    image = f"magik2-build:{recipe_id}"
    name = (
        "magik2-"
        + hashlib.sha256(str(repository.resolve()).encode()).hexdigest()[:12]
        + "-"
        + recipe_id
    )
    result = runner(
        ["container", "list", "--all", "--format", "json"],
        check=True,
        capture_output=True,
        text=True,
    )
    containers = json.loads(result.stdout)
    existing = next((entry for entry in containers if entry["id"] == name), None)
    if existing:
        mounts = existing["configuration"]["mounts"]
        if not any(
            mount["destination"] == "/workspace"
            and Path(mount["source"]).resolve() == repository.resolve()
            for mount in mounts
        ):
            raise RuntimeError("build container belongs to another checkout")
        if existing["status"]["state"] != "running":
            runner(["container", "start", name], check=True)
        return name
    if runner(
        ["container", "image", "inspect", image], check=False, capture_output=True
    ).returncode:
        runner(
            [
                "container",
                "build",
                "--tag",
                image,
                "--file",
                str(recipe),
                str(recipe.parent),
            ],
            check=True,
        )
    cache = Path(
        os.environ.get(
            "MISTER_MAGIK2_BUILD_CACHE", str(Path.home() / ".cache/mister-magik2/cargo")
        )
    )
    mounts = ["--volume", f"{repository.resolve()}:/workspace"]
    for component in ("registry", "git"):
        path = cache / component
        path.mkdir(parents=True, exist_ok=True)
        mounts += ["--volume", f"{path}:/root/.cargo/{component}"]
    runner(
        [
            "container",
            "run",
            "--detach",
            "--name",
            name,
            "--cpus",
            "4",
            "--memory",
            "4g",
            *mounts,
            image,
            "sleep",
            "infinity",
        ],
        check=True,
    )
    return name


def ensure_arm_package(
    package: Path,
    cache_file: Path,
    *,
    runner: Callable = subprocess.run,
    prepare: Callable | None = None,
) -> BuildResult:
    artifact = package / "target" / TARGET / "release" / f"mister-magik2-{package.name}"
    fingerprint = source_fingerprint(package)
    started = time.monotonic()
    if artifact.is_file() and not needs_build(cache_file, fingerprint):
        return BuildResult(
            artifact,
            False,
            int((time.monotonic() - started) * 1000),
            fingerprint=fingerprint,
        )
    repository = package.resolve().parents[1]
    name = (prepare or prepare_container)(repository, runner)
    environment = []
    for variable in ("RUSTFLAGS", "CARGO_ENCODED_RUSTFLAGS"):
        if variable in os.environ:
            environment += ["--env", f"{variable}={os.environ[variable]}"]
    result = runner(
        [
            "container",
            "exec",
            *environment,
            "--workdir",
            f"/workspace/magik2/{package.name}",
            name,
            "cargo",
            "build",
            "--locked",
            "--release",
            "--target",
            TARGET,
        ],
        check=False,
    )
    if result.returncode or not artifact.is_file():
        raise RuntimeError(f"MagiK 2 ARM {package.name} build failed")
    write_build_cache(cache_file, fingerprint, artifact)
    return BuildResult(
        artifact,
        True,
        int((time.monotonic() - started) * 1000),
        fingerprint=fingerprint,
    )


def ensure_arm_probe(
    probe_root: Path,
    cache_file: Path,
    *,
    runner: Callable = subprocess.run,
    prepare: Callable | None = None,
) -> BuildResult:
    prebuilt = os.environ.get("MISTER_MAGIK2_PREBUILT_ARTIFACT")
    if prebuilt:
        artifact = Path(prebuilt).expanduser().resolve()
        if not artifact.is_file():
            raise RuntimeError("MagiK 2 prebuilt probe artifact is unavailable")
        return BuildResult(artifact, False, 0, prebuilt=True)
    return ensure_arm_package(probe_root, cache_file, runner=runner, prepare=prepare)


def ensure_arm_agent() -> Path:
    package = Path(__file__).resolve().parents[2] / "agent"
    return ensure_arm_package(package, package / "target/magik2-build.json").artifact
