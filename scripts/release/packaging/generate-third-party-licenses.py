#!/usr/bin/env python3
# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

"""Generate the shipped Rust dependency inventory for the ARM UI build."""

import json
import subprocess
from pathlib import Path

ROOT = Path(__file__).resolve().parents[3]
MANIFEST = ROOT / "apps/mister/Cargo.toml"
OUTPUT = ROOT / "apps/mister/licenses/RUST-LIBRARIES.txt"
FFMPEG_OUTPUT = ROOT / "apps/mister/licenses/FFMPEG.txt"
FONT_OUTPUT = ROOT / "apps/mister/licenses/PRESS-START-2P.txt"
FFMPEG_BUILD_SOURCE = ROOT / "agent-cli/src/build.rs"
FONT_LICENSE = ROOT / "apps/mister/ui/fonts/PressStart2P-Regular.ttf.license"


def metadata():
    command = [
        "cargo",
        "metadata",
        "--format-version",
        "1",
        "--locked",
        "--manifest-path",
        str(MANIFEST),
        "--filter-platform",
        "armv7-unknown-linux-gnueabihf",
        "--features",
        "ui",
    ]
    return json.loads(subprocess.check_output(command, cwd=ROOT))


def normal_dependency_closure(data):
    nodes = {node["id"]: node for node in data["resolve"]["nodes"]}
    root_id = data["resolve"]["root"]
    seen = {root_id}
    pending = [root_id]
    while pending:
        node = nodes[pending.pop()]
        for dep in node["deps"]:
            if not any(kind["kind"] is None for kind in dep["dep_kinds"]):
                continue
            if dep["pkg"] not in seen:
                seen.add(dep["pkg"])
                pending.append(dep["pkg"])
    return seen


def license_files(package):
    manifest_dir = Path(package["manifest_path"]).parent
    candidates = []
    if package.get("license_file"):
        candidates.append(manifest_dir / package["license_file"])
    for pattern in ("LICENSE*", "COPYING*", "NOTICE*", "COPYRIGHT*"):
        candidates.extend(
            sorted(path for path in manifest_dir.glob(pattern) if path.is_file())
        )
    licenses_dir = manifest_dir / "LICENSES"
    if licenses_dir.is_dir():
        candidates.extend(
            sorted(path for path in licenses_dir.iterdir() if path.is_file())
        )
    result = []
    seen = set()
    for path in candidates:
        if not path.is_file():
            continue
        text = "\n".join(
            line.rstrip() for line in path.read_text(errors="replace").splitlines()
        ).strip()
        if text and text not in seen:
            seen.add(text)
            result.append((path.name, text))
    return result


def main():
    data = metadata()
    closure = normal_dependency_closure(data)
    workspace = set(data["workspace_members"])
    packages = sorted(
        (
            package
            for package in data["packages"]
            if package["id"] in closure
            and package["id"] not in workspace
            and package.get("source") is not None
        ),
        key=lambda package: (package["name"].lower(), package["version"]),
    )
    package_files = {package["id"]: license_files(package) for package in packages}
    expression_files = {}
    for package in packages:
        files = package_files[package["id"]]
        if files and package.get("license"):
            expression_files.setdefault(package["license"], files)
    for package in packages:
        if package_files[package["id"]] or not package.get("license"):
            continue
        exact = expression_files.get(package["license"])
        if exact:
            package_files[package["id"]] = exact
            continue
        tokens = [
            token
            for token in package["license"]
            .replace("(", " ")
            .replace(")", " ")
            .replace("/", " ")
            .split()
            if token not in {"AND", "OR", "WITH"}
        ]
        borrowed = []
        for token in tokens:
            donor = next(
                (
                    files
                    for candidate in packages
                    if token in (candidate.get("license") or "")
                    and (files := package_files[candidate["id"]])
                ),
                None,
            )
            if donor:
                borrowed.extend(donor)
        package_files[package["id"]] = borrowed

    bodies = []
    body_ids = {}
    entries = []
    for package in packages:
        refs = []
        for filename, body in package_files[package["id"]]:
            body_id = body_ids.get(body)
            if body_id is None:
                body_id = len(bodies) + 1
                body_ids[body] = body_id
                bodies.append((body_id, filename, body))
            refs.append(str(body_id))
        if not refs:
            raise SystemExit(
                f"no full license text found for {package['name']} {package['version']}"
            )
        entries.append(
            f"{package['name']} {package['version']}\n"
            f"License: {package.get('license') or 'See bundled license file'}\n"
            f"Authors: {', '.join(package.get('authors') or []) or 'See source and notices'}\n"
            f"Source: {package.get('repository') or package.get('homepage') or package['source']}\n"
            f"Full license text: {', '.join('Text ' + ref for ref in refs)}"
        )
    header = (
        "RUST LIBRARIES\n\n"
        "Generated by scripts/release/packaging/generate-third-party-licenses.py from Cargo.lock for "
        "armv7-unknown-linux-gnueabihf with feature ui. Only normal runtime "
        "dependencies are included; build, development, host-only, and inactive optional "
        "dependencies are excluded. Identical license bodies are deduplicated.\n\n"
    )
    inventory = "\n\n".join(entries)
    texts = "\n\n".join(
        f"{'=' * 72}\nTEXT {body_id}: {filename}\n{'=' * 72}\n\n{body}"
        for body_id, filename, body in bodies
    )
    OUTPUT.parent.mkdir(parents=True, exist_ok=True)
    OUTPUT.write_text(header + inventory + "\n\n" + texts + "\n")
    configure = FFMPEG_BUILD_SOURCE.read_text()
    for required in (
        "--disable-autodetect",
        "--disable-everything",
        "--disable-shared",
        "CONFIG_GPL 0",
        "CONFIG_VERSION3 0",
        "CONFIG_NONFREE 0",
    ):
        if required not in configure:
            raise SystemExit(f"FFmpeg license gate: missing {required}")
    for forbidden in ("--enable-gpl", "--enable-version3", "--enable-nonfree"):
        if forbidden in configure:
            raise SystemExit(f"FFmpeg license gate: forbidden {forbidden}")
    if not FFMPEG_OUTPUT.is_file():
        raise SystemExit("missing vendored FFmpeg LGPL notice")
    FONT_OUTPUT.write_text(FONT_LICENSE.read_text())
    print(
        f"wrote {OUTPUT.relative_to(ROOT)}: {len(packages)} packages, {len(bodies)} unique license texts"
    )


if __name__ == "__main__":
    main()
