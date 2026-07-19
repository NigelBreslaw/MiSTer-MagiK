#!/usr/bin/env python3
# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

"""Report local development readiness without contacting a MiSTer."""

from __future__ import annotations

import argparse
import json
import os
from pathlib import Path
import platform
import re
import shutil
import subprocess
import sys
from typing import Any


SCHEMA = "mister-magik-doctor-v1"
SCOPES = ("full-host", "desktop", "arm", "docs", "device")


def command_output(command: list[str]) -> str | None:
    try:
        result = subprocess.run(
            command,
            check=False,
            stdout=subprocess.PIPE,
            stderr=subprocess.DEVNULL,
            text=True,
            timeout=5,
        )
    except (OSError, subprocess.TimeoutExpired):
        return None
    return result.stdout.strip() if result.returncode == 0 else None


class Report:
    def __init__(self, scope: str) -> None:
        self.scope = scope
        self.checks: list[dict[str, Any]] = []

    def add(
        self,
        check_id: str,
        ok: bool,
        detail: str,
        remediation: str | None = None,
        *,
        required: bool = True,
        warning: bool = False,
    ) -> None:
        status = "pass" if ok else ("warn" if warning else "fail")
        self.checks.append(
            {
                "id": check_id,
                "status": status,
                "required": required,
                "detail": detail,
                "remediation": remediation if not ok else None,
            }
        )

    @property
    def ok(self) -> bool:
        return not any(
            check["required"] and check["status"] == "fail"
            for check in self.checks
        )

    def value(self) -> dict[str, Any]:
        return {
            "schema": SCHEMA,
            "scope": self.scope,
            "ok": self.ok,
            "requires_device": self.scope == "device",
            "device_probe": "not_attempted",
            "checks": self.checks,
        }


def require_command(report: Report, name: str, remediation: str) -> bool:
    path = shutil.which(name)
    report.add(
        f"command-{name}",
        path is not None,
        path or f"{name} not found",
        remediation,
    )
    return path is not None


def check_rust_toolchain(root: Path, report: Report, *, require_arm: bool) -> None:
    toolchain_path = root / "apps/mister/rust-toolchain.toml"
    text = toolchain_path.read_text(encoding="utf-8") if toolchain_path.is_file() else ""
    match = re.search(r'^channel = "([^"]+)"$', text, re.MULTILINE)
    channel = match.group(1) if match else None
    if not channel:
        report.add(
            "rust-toolchain",
            False,
            "pinned Rust channel is unreadable",
            "restore apps/mister/rust-toolchain.toml",
        )
        return
    installed = command_output(["rustup", "toolchain", "list"]) or ""
    report.add(
        "rust-toolchain",
        any(line.split("-", 1)[0] == channel or line.startswith(channel) for line in installed.splitlines()),
        f"pinned channel {channel}",
        f"run: rustup toolchain install {channel} --component rustfmt --component clippy",
    )
    components = command_output(
        ["rustup", "component", "list", "--toolchain", channel]
    ) or ""
    for component in ("rustfmt", "clippy"):
        installed_component = any(
            line.startswith(f"{component}-") and line.endswith("(installed)")
            for line in components.splitlines()
        )
        report.add(
            f"rust-component-{component}",
            installed_component,
            f"{component} for {channel}",
            f"run: rustup component add {component} --toolchain {channel}",
        )
    if require_arm:
        targets = command_output(
            ["rustup", "target", "list", "--toolchain", channel]
        ) or ""
        arm_target = "armv7-unknown-linux-gnueabihf"
        report.add(
            "rust-target-armv7",
            any(
                line.startswith(arm_target) and line.endswith("(installed)")
                for line in targets.splitlines()
            ),
            f"{arm_target} for {channel}",
            f"run: rustup target add {arm_target} --toolchain {channel}",
        )


def check_desktop(root: Path, report: Report) -> None:
    for name in ("github-app", "material-icon-theme"):
        path = root / "apps/desktop/vendor" / name
        report.add(
            f"desktop-submodule-{name}",
            (path / ".git").exists(),
            str(path),
            "run: git submodule update --init apps/desktop/vendor/github-app apps/desktop/vendor/material-icon-theme",
        )


def check_docs(root: Path, report: Report) -> None:
    node = command_output(["node", "--version"])
    major = None
    if node:
        match = re.match(r"v(\d+)", node)
        major = int(match.group(1)) if match else None
    report.add(
        "node-version",
        major is not None and major >= 22,
        node or "Node.js unavailable",
        "install Node.js 22 or newer with Corepack",
    )
    pnpm = command_output(
        ["env", "COREPACK_ENABLE_NETWORK=0", "corepack", "pnpm", "--version"]
    )
    report.add(
        "pnpm-version",
        pnpm == "11.10.0",
        pnpm or "Corepack pnpm unavailable",
        "run: corepack prepare pnpm@11.10.0 --activate",
    )
    modules = root / "documentation/node_modules"
    ready = (modules / ".pnpm").is_dir() and os.access(modules / ".bin/astro", os.X_OK)
    report.add(
        "documentation-dependencies",
        ready,
        str(modules),
        "run: pnpm --dir documentation install --frozen-lockfile",
    )


def check_output(root: Path, report: Report) -> None:
    outputs = ("build", "dist", "outputs", "target", "documentation/dist")
    build = root / "build"
    writable = os.access(build if build.exists() else root, os.W_OK)
    report.add(
        "build-output-writable",
        writable,
        str(build),
        "make the repository build output writable",
    )
    for output in outputs:
        sentinel = f"{output}/.mister-magik-doctor"
        ignored = subprocess.run(
            ["git", "-C", str(root), "check-ignore", "-q", sentinel],
            check=False,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
        ).returncode == 0
        report.add(
            f"output-ignored-{output.replace('/', '-')}",
            ignored,
            sentinel,
            f"add {output}/ to the repository ignore policy",
        )


def build_report(root: Path, scope: str) -> Report:
    report = Report(scope)
    require_command(report, "python3", "install Python 3")
    require_command(report, "git", "install Git")

    if scope in {"full-host", "desktop", "arm"}:
        require_command(report, "cargo", "install Rust through rustup")
        require_command(report, "rustup", "install rustup")
        check_rust_toolchain(root, report, require_arm=scope == "arm")
    if scope in {"full-host", "desktop"}:
        check_desktop(root, report)
    if scope in {"full-host", "docs"}:
        require_command(report, "node", "install Node.js 22 or newer")
        require_command(report, "corepack", "install Node.js with Corepack")
        check_docs(root, report)
    if scope == "arm":
        if platform.system() == "Darwin" and platform.machine() == "arm64":
            require_command(report, "container", "install and start Apple container")
        else:
            require_command(report, "cross", "install cross 0.2.5")
            require_command(report, "docker", "install and start Docker")
    if scope == "device":
        mister = root / "scripts/mister"
        report.add(
            "device-wrapper",
            os.access(mister, os.X_OK),
            str(mister),
            "restore the executable scripts/mister wrapper",
        )
        report.add(
            "device-network",
            False,
            "network probe intentionally not attempted",
            required=False,
            warning=True,
        )
    entrypoints = ["scripts/validate", "scripts/dev-rust"]
    if scope == "full-host":
        entrypoints.append("scripts/test-host-tools.sh")
    for entrypoint in entrypoints:
        path = root / entrypoint
        report.add(
            f"entrypoint-{entrypoint.replace('/', '-')}",
            os.access(path, os.X_OK),
            str(path),
            f"restore executable permissions on {entrypoint}",
        )
    check_output(root, report)
    return report


def print_human(value: dict[str, Any]) -> None:
    for check in value["checks"]:
        label = check["status"].upper()
        print(f"{label:4} {check['id']}: {check['detail']}")
        if check["remediation"]:
            print(f"     {check['remediation']}")
    print(f"doctor: {'ready' if value['ok'] else 'not ready'} ({value['scope']})")


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--scope", choices=SCOPES, default="full-host")
    parser.add_argument("--json", action="store_true")
    parser.add_argument("--root", type=Path, default=Path(__file__).resolve().parents[2], help=argparse.SUPPRESS)
    args = parser.parse_args()
    value = build_report(args.root.resolve(), args.scope).value()
    if args.json:
        print(json.dumps(value, sort_keys=True))
    else:
        print_human(value)
    return 0 if value["ok"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
