#!/usr/bin/env python3
# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

"""Run the bounded, bootstrap-free MiSTer MagiK pre-commit gate."""

from __future__ import annotations

import argparse
import os
from pathlib import Path, PurePosixPath
import subprocess
import sys
from typing import NoReturn, Sequence

EXPECTED_NAME = "Nigel Breslaw"
EXPECTED_EMAIL = "nigel.breslaw@gmail.com"

FORBIDDEN_ARCHIVE_SUFFIXES = {
    ".7z",
    ".bz2",
    ".gz",
    ".key",
    ".p12",
    ".pfx",
    ".rar",
    ".tar",
    ".tgz",
    ".xz",
    ".zip",
}
PRIVATE_IMAGE_SUFFIXES = {".gif", ".jpeg", ".jpg", ".png", ".webp"}
FORBIDDEN_NAMES = {"credentials", "id_ed25519", "id_rsa", "secrets"}

CLASSIFIED_PREFIXES = (
    ".github",
    ".githooks",
    "LICENSES",
    "agent-cli",
    "apps/desktop",
    "apps/framebuffer-lab",
    "apps/framebuffer-scene-lab",
    "apps/mister",
    "crates",
    "docs",
    "documentation",
    "history",
    "mister/platform/contracts",
    "mister/platform/fpga",
    "mister/platform/kernel",
    "mister/platform/runtime",
    "mister/tools/agent",
    "mister/tools/manager",
    "private",
    "scripts",
    "tools",
)

CRATE_FORMATTERS = (
    (
        "framebuffer-scene-lab.format",
        "apps/framebuffer-scene-lab",
        "apps/framebuffer-scene-lab/Cargo.toml",
    ),
    (
        "framebuffer-lab.format",
        "apps/framebuffer-lab",
        "apps/framebuffer-lab/Cargo.toml",
    ),
    (
        "agent-protocol.format",
        "crates/agent-protocol",
        "crates/agent-protocol/Cargo.toml",
    ),
    (
        "framebuffer-stream.format",
        "crates/framebuffer-stream",
        "crates/framebuffer-stream/Cargo.toml",
    ),
    (
        "framebuffer-scenes.format",
        "crates/framebuffer-scenes",
        "crates/framebuffer-scenes/Cargo.toml",
    ),
    (
        "screenshot-parade.format",
        "crates/screenshot-parade",
        "crates/screenshot-parade/Cargo.toml",
    ),
    (
        "latch-contract.format",
        "mister/platform/contracts/latch",
        "mister/platform/contracts/latch/Cargo.toml",
    ),
    ("magik-core.format", "crates/magik-core", "crates/magik-core/Cargo.toml"),
    (
        "media-contract.format",
        "crates/media-contract",
        "crates/media-contract/Cargo.toml",
    ),
    ("mister-agent.format", "mister/tools/agent", "mister/tools/agent/Cargo.toml"),
    ("mister-ini.format", "crates/mister-ini", "crates/mister-ini/Cargo.toml"),
    (
        "mister-manager.format",
        "mister/tools/manager",
        "mister/tools/manager/Cargo.toml",
    ),
    (
        "mister-runtime.format",
        "mister/platform/runtime",
        "mister/platform/runtime/Cargo.toml",
    ),
    (
        "scanout-contract.format",
        "mister/platform/contracts/scanout",
        "mister/platform/contracts/scanout/Cargo.toml",
    ),
)

APP_FORMAT_PREFIXES = (
    "apps/mister/src",
    "apps/mister/ui",
    "apps/mister/ui-generated",
    "apps/mister/examples",
    "apps/mister/.cargo",
)
APP_FORMAT_FILES = {
    "apps/mister/Cargo.lock",
    "apps/mister/Cargo.toml",
    "apps/mister/Cross.toml",
    "apps/mister/Dockerfile.cross-armv7",
    "apps/mister/build.rs",
    "apps/mister/rust-toolchain.toml",
}
PIXEL_TEXT_CONTRACT_PATHS = {
    "scripts/checks/check-pixel-text-contract.py",
    "scripts/checks/pre-commit.py",
    "scripts/tests/test-pixel-text-contract.py",
}
TEXT_SUFFIXES = {
    ".json",
    ".md",
    ".py",
    ".rs",
    ".sh",
    ".slint",
    ".toml",
    ".txt",
    ".yaml",
    ".yml",
}
DROPPED_FRAME_LEGACY_MARKER = "dropped-frame-legacy-fixture"
DEPRECATED_DROPPED_FRAME_TERMS = (
    "repeated_" + "refreshes",
    "skipped_" + "refreshes",
    "repeated_" + "presentations",
    "repeated-" + "refreshes",
)


class GateError(Exception):
    """A deterministic pre-commit rejection."""


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("--repository", type=Path, required=True)
    return parser.parse_args()


def run(
    repository: Path,
    args: Sequence[str],
    *,
    input_bytes: bytes | None = None,
    allowed_codes: tuple[int, ...] = (0,),
    environment: dict[str, str] | None = None,
) -> subprocess.CompletedProcess[bytes]:
    try:
        result = subprocess.run(
            args,
            cwd=repository,
            input=input_bytes,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            check=False,
            env=environment,
        )
    except OSError as error:
        raise GateError(f"cannot run {args[0]}: {error}") from error
    if result.returncode not in allowed_codes:
        detail = (result.stderr or result.stdout).decode(errors="replace").strip()
        command = " ".join(args)
        raise GateError(
            f"command_failed: {command} exited {result.returncode}"
            + (f"\n{detail}" if detail else "")
        )
    return result


def git(
    repository: Path,
    args: Sequence[str],
    *,
    input_bytes: bytes | None = None,
    allowed_codes: tuple[int, ...] = (0,),
    environment: dict[str, str] | None = None,
) -> subprocess.CompletedProcess[bytes]:
    return run(
        repository,
        ["git", *args],
        input_bytes=input_bytes,
        allowed_codes=allowed_codes,
        environment=environment,
    )


def staged_paths(repository: Path) -> list[str]:
    output = git(
        repository,
        ["diff", "--cached", "--name-only", "-z", "--diff-filter=ACMRD"],
    ).stdout
    return sorted(
        os.fsdecode(value)
        for value in output.split(b"\0")
        if value
    )


def is_classified(path: str) -> bool:
    value = PurePosixPath(path)
    first = value.parts[0] if value.parts else ""
    if path in {"Cargo.toml", "Cargo.lock"}:
        return True
    if len(value.parts) == 1 or value.name == "AGENTS.md":
        return True
    if first.startswith(".") and first not in {".github", ".githooks"}:
        return True
    return any(
        path == prefix or path.startswith(f"{prefix}/")
        for prefix in CLASSIFIED_PREFIXES
    )


def check_classification(paths: Sequence[str]) -> None:
    unknown = [path for path in paths if not is_classified(path)]
    if unknown:
        raise GateError(
            "unclassified changed paths: "
            + ", ".join(unknown)
            + "; add them to the typed impact map"
        )


def config_value(repository: Path, key: str) -> str:
    result = git(
        repository,
        ["config", "--get", key],
        allowed_codes=(0, 1),
    )
    return result.stdout.decode(errors="replace").strip() if result.returncode == 0 else ""


def check_identity(repository: Path) -> None:
    actual_name = config_value(repository, "user.name")
    actual_email = config_value(repository, "user.email")
    if actual_name != EXPECTED_NAME or actual_email != EXPECTED_EMAIL:
        raise GateError(
            "git_identity_mismatch: expected "
            f"{EXPECTED_NAME} <{EXPECTED_EMAIL}>; "
            f"got {actual_name} <{actual_email}>"
        )


def forbidden_path(path: str) -> bool:
    value = PurePosixPath(path)
    name = value.name
    suffix = value.suffix.lower()
    return (
        path.startswith("private/test-fixtures/")
        or ".wrangler" in value.parts
        or name == ".env"
        or name.startswith(".env.")
        or (path.startswith("private/") and suffix in PRIVATE_IMAGE_SUFFIXES)
        or suffix in FORBIDDEN_ARCHIVE_SUFFIXES
        or name in FORBIDDEN_NAMES
        or name.startswith("credentials.")
        or name.startswith("secrets.")
    )


def check_forbidden_and_ignored(repository: Path, paths: Sequence[str]) -> None:
    for path in paths:
        if forbidden_path(path):
            raise GateError(f"staged_git_forbidden: {path}")
    if not paths:
        return
    encoded = b"\0".join(os.fsencode(path) for path in paths) + b"\0"
    ignored = git(
        repository,
        ["check-ignore", "--no-index", "-z", "--stdin"],
        input_bytes=encoded,
        allowed_codes=(0, 1),
    )
    if ignored.returncode == 0:
        first = next((item for item in ignored.stdout.split(b"\0") if item), b"")
        raise GateError(f"staged_git_ignored: {os.fsdecode(first)}")


def check_dropped_frame_terminology(repository: Path, paths: Sequence[str]) -> None:
    for path in paths:
        value = PurePosixPath(path)
        if (
            path.startswith(("history/", "reference/"))
            or value.suffix.lower() not in TEXT_SUFFIXES
        ):
            continue
        staged = git(
            repository,
            ["show", f":{path}"],
            allowed_codes=(0, 128),
        )
        if staged.returncode != 0:
            continue
        lines = staged.stdout.decode(errors="replace").splitlines()
        for line_number, line in enumerate(lines, start=1):
            previous = lines[line_number - 2] if line_number > 1 else ""
            if DROPPED_FRAME_LEGACY_MARKER in line or DROPPED_FRAME_LEGACY_MARKER in previous:
                continue
            for term in DEPRECATED_DROPPED_FRAME_TERMS:
                if term in line:
                    raise GateError(
                        f"deprecated_dropped_frame_term: {path}:{line_number}: {term}"
                    )


def staged_submodules(repository: Path, paths: Sequence[str]) -> list[str]:
    wanted = set(paths)
    output = git(repository, ["ls-files", "--stage", "-z"]).stdout
    submodules = []
    for record in output.split(b"\0"):
        if not record or b"\t" not in record:
            continue
        metadata, raw_path = record.split(b"\t", 1)
        fields = metadata.split()
        path = os.fsdecode(raw_path)
        if (
            len(fields) == 3
            and fields[0] == b"160000"
            and fields[2] == b"0"
            and path in wanted
        ):
            submodules.append(path)
    return sorted(submodules)


def check_submodules(repository: Path, paths: Sequence[str]) -> None:
    submodule_environment = os.environ.copy()
    submodule_environment.pop("GIT_INDEX_FILE", None)
    for path in staged_submodules(repository, paths):
        submodule = repository / path
        status = git(
            submodule,
            ["status", "--porcelain"],
            environment=submodule_environment,
        ).stdout
        if status:
            raise GateError(f"staged_git_dirty_submodule: {path}")
        if path != "private/magik-cloud":
            continue
        upstream = git(
            submodule,
            ["rev-parse", "@{u}"],
            allowed_codes=(0, 1, 128),
            environment=submodule_environment,
        )
        if upstream.returncode != 0:
            raise GateError("staged_git_private_submodule_has_no_upstream")
        head = git(
            submodule,
            ["rev-parse", "HEAD"],
            environment=submodule_environment,
        ).stdout.strip()
        ancestor = git(
            submodule,
            [
                "merge-base",
                "--is-ancestor",
                os.fsdecode(head),
                upstream.stdout.decode().strip(),
            ],
            allowed_codes=(0, 1),
            environment=submodule_environment,
        )
        if ancestor.returncode != 0:
            raise GateError("staged_git_private_submodule_must_be_pushed_first")


def is_app_format_path(path: str) -> bool:
    value = PurePosixPath(path)
    return value.name != "AGENTS.md" and (
        path in APP_FORMAT_FILES
        or any(
            path == prefix or path.startswith(f"{prefix}/")
            for prefix in APP_FORMAT_PREFIXES
        )
    )


def formatters(paths: Sequence[str]) -> list[tuple[str, str]]:
    selected: dict[str, str] = {}
    for path in paths:
        if path == "agent-cli" or path.startswith("agent-cli/"):
            selected["agent-cli.format"] = "agent-cli/Cargo.toml"
        if path == "crates/catalog" or path.startswith("crates/catalog/"):
            selected["catalog.format"] = "crates/catalog/Cargo.toml"
        if is_app_format_path(path):
            selected["app.format"] = "apps/mister/Cargo.toml"
        for operation_id, prefix, manifest in CRATE_FORMATTERS:
            if path == prefix or path.startswith(f"{prefix}/"):
                selected[operation_id] = manifest
    return sorted(selected.items())


def shell_paths(repository: Path, paths: Sequence[str]) -> list[str]:
    selected = []
    for path in paths:
        try:
            first_line = (repository / path).read_text().splitlines()[0]
        except (IndexError, OSError, UnicodeError):
            continue
        if first_line in {"#!/bin/bash", "#!/usr/bin/env bash"}:
            selected.append(path)
    return selected


def needs_pixel_text_contract(paths: Sequence[str]) -> bool:
    return any(
        path in PIXEL_TEXT_CONTRACT_PATHS
        or (
            path.startswith("apps/mister/ui/")
            and PurePosixPath(path).suffix == ".slint"
        )
        for path in paths
    )


def execute(repository: Path) -> None:
    paths = staged_paths(repository)
    check_classification(paths)
    shells = shell_paths(repository, paths)
    cargo_formatters = formatters(paths)
    pixel_text_contract = needs_pixel_text_contract(paths)
    planned = 5 + len(shells) + len(cargo_formatters) + int(pixel_text_contract)
    print(f"pre-commit: {planned} fast checks planned (0%)")

    check_identity(repository)
    check_forbidden_and_ignored(repository, paths)
    check_dropped_frame_terminology(repository, paths)
    check_submodules(repository, paths)
    git(repository, ["diff", "--cached", "--check"])
    run(
        repository,
        [sys.executable, "scripts/checks/check-unified-agent-surface.py", str(repository)],
    )
    for path in shells:
        run(repository, ["bash", "-n", path])
    if pixel_text_contract:
        run(
            repository,
            [
                sys.executable,
                "scripts/checks/check-pixel-text-contract.py",
                "--repository",
                str(repository),
                "--staged",
            ],
        )
    for _, manifest in cargo_formatters:
        run(repository, ["cargo", "fmt", "--manifest-path", manifest, "--check"])

    print("pre-commit: passed (100%)")


def fail(message: str) -> NoReturn:
    print(f"error: {message}", file=sys.stderr)
    raise SystemExit(1)


def main() -> None:
    args = parse_args()
    repository = args.repository.resolve()
    try:
        execute(repository)
    except GateError as error:
        fail(str(error))


if __name__ == "__main__":
    main()
