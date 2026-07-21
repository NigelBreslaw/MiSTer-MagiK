#!/usr/bin/env python3
# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

from __future__ import annotations

from pathlib import Path
import re


ROOT = Path(__file__).resolve().parents[2]
WORKFLOWS = ROOT / ".github/workflows"
CACHE_USE = re.compile(r"uses:\s*actions/cache(?:/(?:restore|save))?@v(\d+)")
KEY_LINE = re.compile(r"^\s*key:\s*(.+)$", re.MULTILINE)
BINARY_PREFIXES = (
    "target-arm-",
    "target-agent-arm-",
    "cross-",
    "ffmpeg-minimal-",
    "main-mister-bin-",
    "main-toolchain-",
)


def fail(message: str) -> None:
    raise SystemExit(f"cache contract: {message}")


def main() -> int:
    texts = {path.name: path.read_text(encoding="utf-8") for path in sorted(WORKFLOWS.glob("*.yml"))}
    combined = "\n".join(texts.values())

    versions = [int(match.group(1)) for text in texts.values() for match in CACHE_USE.finditer(text)]
    if not versions:
        fail("no actions/cache uses found")
    if min(versions) < 6:
        fail(f"actions/cache older than v6 remains: {versions}")
    for forbidden in ("ci-clippy", "target-clippy", "target-arm-dist", "cross-custom-rust"):
        if forbidden in combined:
            fail(f"forbidden legacy identity remains: {forbidden}")
    if re.search(r"hashFiles\([^)]*\.github/workflows", combined):
        fail("a cache key hashes a workflow file")

    keys = [match.group(1) for text in texts.values() for match in KEY_LINE.finditer(text)]
    for key in keys:
        if any(prefix in key for prefix in BINARY_PREFIXES):
            if "${{ runner.os }}" not in key or "${{ runner.arch }}" not in key:
                fail(f"binary cache lacks OS/architecture: {key}")
            if "-v2-" not in key:
                fail(f"binary cache lacks schema v2: {key}")

    rust = texts["rust-arm.yml"]
    required_host_fragments = ("steps.cache-id.outputs.cargo_host",)
    for fragment in required_host_fragments:
        if fragment not in rust:
            fail(f"host cache is missing {fragment}")
    for forbidden in (
        "target-host-",
        "Cache host build outputs",
        "apps/mister/target/debug",
        "crates/catalog/target/debug",
        "mister/tools/agent/target/debug",
        "mister/tools/host/target/debug",
        "desktop-docs",
        "apps/desktop/target/debug",
        "libfontconfig1-dev",
        "scripts/agent verify --paths desktop",
    ):
        if forbidden in rust:
            fail(f"rust-arm workflow still validates the desktop app: {forbidden}")
    if rust.count("steps.cache-id.outputs.cross_abi") < 6:
        fail("ARM restore keys are not consistently scoped to the cross ABI")

    distribution = texts["distribution.yml"]
    if "uses: actions/cache/restore@v6" not in distribution:
        fail("distribution ARM target must be restore-only")
    if "target-arm-v2-" not in distribution:
        fail("distribution does not restore the normal ARM target cache")
    for fragment in (
        "packages: read",
        "GHCR_TOKEN: ${{ secrets.GITHUB_TOKEN }}",
        'docker pull "${{ steps.cache-id.outputs.cross_image }}"',
        "apps/mister/target/ffmpeg-minimal/armv7",
        "ffmpeg-minimal-v2-${{ runner.os }}-${{ runner.arch }}-armv7-unknown-linux-gnueabihf-8.1.2-${{ steps.cache-id.outputs.cross_abi }}-${{ steps.cache-id.outputs.ffmpeg }}",
    ):
        if fragment not in distribution:
            fail(f"distribution is missing canonical cross-image setup: {fragment}")
    if distribution.index('docker pull "${{ steps.cache-id.outputs.cross_image }}"') > distribution.index(
        "scripts/agent build runtime-device"
    ):
        fail("distribution pulls the cross image after the ARM build starts")

    cross_toml = (ROOT / "apps/mister/Cross.toml").read_text(encoding="utf-8")
    image_match = re.search(r'^image\s*=\s*"([^"]+)"', cross_toml, re.MULTILINE)
    if not image_match or not image_match.group(1).startswith("ghcr.io/"):
        fail("Cross.toml must contain the canonical GHCR image")
    for workflow in ("rust-arm.yml", "distribution.yml", "cross-image.yml"):
        if "ci-cache-identity.py" not in texts[workflow]:
            fail(f"{workflow} does not consume the canonical cache identity")

    cross_image = texts["cross-image.yml"]
    for fragment in (
        "if: github.ref != 'refs/heads/main'",
        "group: mister-magik-cross-image",
        "cancel-in-progress: false",
        "sha256sum apps/mister/Dockerfile.cross-armv7",
        "ghcr.io/nigelbreslaw/mister-magik-cross-armv7:ubuntu20-$dockerfile_hash",
        'docker manifest inspect "$IMAGE"',
        "content-versioned images are immutable",
    ):
        if fragment not in cross_image:
            fail(f"cross-image workflow is missing write-once protection: {fragment}")

    ffmpeg_helper = (ROOT / "apps/mister/scripts/build-minimal-ffmpeg.sh").read_text(
        encoding="utf-8"
    )
    if '"$HERE/../../scripts/checks/ci-cache-identity.py"' not in ffmpeg_helper:
        fail("minimal FFmpeg helper does not resolve the canonical cache identity from repo root")

    platform = texts["platform-bundle.yml"]
    fpga = platform.split("  build-fpga:\n", 1)[1].split("\n  assemble:\n", 1)[0]
    if "actions/cache" in fpga or "prepare-quartus" in fpga:
        fail("Quartus still uses GitHub cache or a duplicate preparation job")
    if fpga.count("scripts/quartus-r2-cache.sh") != 2:
        fail("Quartus workflow must have exactly one R2 restore and one save")
    for secret in (
        "QUARTUS_R2_READ_ACCESS_KEY_ID",
        "QUARTUS_R2_READ_SECRET_ACCESS_KEY",
        "QUARTUS_R2_WRITE_ACCESS_KEY_ID",
        "QUARTUS_R2_WRITE_SECRET_ACCESS_KEY",
    ):
        if secret not in fpga:
            fail(f"Quartus workflow is missing {secret}")

    print("CI cache contract tests ok")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
