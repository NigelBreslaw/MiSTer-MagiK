#!/usr/bin/env python3
# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

"""Generate and verify the canonical MiSTer MagiK platform manifest."""

from __future__ import annotations

import argparse
import hashlib
import re
import sys
from pathlib import Path

FORMAT = "mister-magik-platform-v1"
DEVICE_PATHS = {
    "main": "/media/fat/MiSTer_MagiK",
    "gui": "/media/fat/mister-magik/mister-magik-fb",
    "catalog_builder": "/media/fat/mister-magik/mister-magik-catalog-builder",
    "scanout_module": "/media/fat/mister-magik/mister_magik_scanout_slots.ko",
    "scanout_metadata": "/media/fat/mister-magik/mister_magik_scanout_slots.metadata.txt",
    "latch_rbf": "/media/fat/mister-magik/fpga/menu-magik-vblank-latch.rbf",
    "latch_metadata": "/media/fat/mister-magik/fpga/menu-magik-vblank-latch.metadata.txt",
}
FIELDS = (
    "format",
    "main_path", "gui_path", "catalog_builder_path",
    "scanout_module_path", "scanout_metadata_path",
    "latch_rbf_path", "latch_metadata_path",
    "main_sha256", "gui_sha256", "catalog_builder_sha256",
    "scanout_module_sha256", "scanout_metadata_sha256",
    "latch_rbf_sha256", "latch_metadata_sha256", "platform_contract_sha256",
    "main_revision", "magik_revision", "menu_revision",
)
HEX40 = re.compile(r"[0-9a-f]{40}")
HEX64 = re.compile(r"[0-9a-f]{64}")


class ManifestError(ValueError):
    pass


def digest(path: Path) -> str:
    value = hashlib.sha256()
    with path.open("rb") as source:
        for chunk in iter(lambda: source.read(1024 * 1024), b""):
            value.update(chunk)
    return value.hexdigest()


def parse_fields(path: Path, *, exact: tuple[str, ...] | None = None) -> dict[str, str]:
    if not path.is_file():
        raise ManifestError(f"missing file: {path}")
    result: dict[str, str] = {}
    for number, raw in enumerate(path.read_text().splitlines(), 1):
        if not raw or raw.startswith("#"):
            continue
        if "=" not in raw:
            raise ManifestError(f"{path}:{number}: malformed line")
        key, value = raw.split("=", 1)
        if not key or not value:
            raise ManifestError(f"{path}:{number}: empty key or value")
        if key in result:
            raise ManifestError(f"{path}:{number}: duplicate field {key}")
        result[key] = value
    if exact is not None:
        missing = set(exact) - result.keys()
        extra = result.keys() - set(exact)
        if missing:
            raise ManifestError("missing manifest fields: " + ", ".join(sorted(missing)))
        if extra:
            raise ManifestError("unknown manifest fields: " + ", ".join(sorted(extra)))
    return result


def require_hex(name: str, value: str, pattern: re.Pattern[str]) -> None:
    if pattern.fullmatch(value) is None:
        raise ManifestError(f"invalid {name}: {value}")


def device_file(root: Path | None, device_path: str) -> Path:
    if root is None:
        return Path(device_path)
    prefix = "/media/fat/"
    if not device_path.startswith(prefix):
        raise ManifestError(f"path is outside /media/fat: {device_path}")
    return root / device_path.removeprefix(prefix)


def validate_metadata(
    module: Path,
    module_metadata: Path,
    rbf: Path,
    rbf_metadata: Path,
) -> tuple[str, str]:
    module_fields = parse_fields(module_metadata)
    rbf_fields = parse_fields(rbf_metadata)
    module_hash = digest(module)
    rbf_hash = digest(rbf)
    if module_fields.get("module_sha256") != module_hash:
        raise ManifestError("scanout metadata does not match module")
    if rbf_fields.get("rbf_sha256") != rbf_hash:
        raise ManifestError("latch metadata does not match RBF")
    contracts = {
        module_fields.get("platform_contract_sha256", ""),
        rbf_fields.get("platform_contract_sha256", ""),
    }
    if len(contracts) != 1:
        raise ManifestError("mixed framebuffer platform-contract hashes")
    contract = contracts.pop()
    require_hex("platform_contract_sha256", contract, HEX64)
    menu_revision = rbf_fields.get("source_commit", "")
    require_hex("menu_revision", menu_revision, HEX40)
    return contract, menu_revision


def generate(args: argparse.Namespace) -> None:
    artifacts = {
        "main": args.main,
        "gui": args.gui,
        "catalog_builder": args.catalog_builder,
        "scanout_module": args.scanout_module,
        "scanout_metadata": args.scanout_metadata,
        "latch_rbf": args.latch_rbf,
        "latch_metadata": args.latch_metadata,
    }
    for name, path in artifacts.items():
        if not path.is_file():
            raise ManifestError(f"missing {name}: {path}")
    require_hex("main_revision", args.main_revision, HEX40)
    require_hex("magik_revision", args.magik_revision, HEX40)
    contract, menu_revision = validate_metadata(
        args.scanout_module,
        args.scanout_metadata,
        args.latch_rbf,
        args.latch_metadata,
    )
    values = {
        "format": FORMAT,
        **{f"{name}_path": path for name, path in DEVICE_PATHS.items()},
        **{f"{name}_sha256": digest(path) for name, path in artifacts.items()},
        "platform_contract_sha256": contract,
        "main_revision": args.main_revision,
        "magik_revision": args.magik_revision,
        "menu_revision": menu_revision,
    }
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text("".join(f"{field}={values[field]}\n" for field in FIELDS))
    verify_manifest(args.output, artifact_root=args.artifact_root)
    print(args.output)


def verify_manifest(path: Path, *, artifact_root: Path | None = None) -> None:
    fields = parse_fields(path, exact=FIELDS)
    if fields["format"] != FORMAT:
        raise ManifestError(f"unsupported manifest format: {fields['format']}")
    for name, expected in DEVICE_PATHS.items():
        if fields[f"{name}_path"] != expected:
            raise ManifestError(f"incorrect {name}_path")
    for name in DEVICE_PATHS:
        require_hex(f"{name}_sha256", fields[f"{name}_sha256"], HEX64)
    require_hex("platform_contract_sha256", fields["platform_contract_sha256"], HEX64)
    for name in ("main_revision", "magik_revision", "menu_revision"):
        require_hex(name, fields[name], HEX40)
    if artifact_root is None:
        return
    files = {name: device_file(artifact_root, DEVICE_PATHS[name]) for name in DEVICE_PATHS}
    for name, artifact in files.items():
        if not artifact.is_file():
            raise ManifestError(f"missing installed {name}: {artifact}")
        actual = digest(artifact)
        if actual != fields[f"{name}_sha256"]:
            raise ManifestError(f"incorrect installed {name} hash")
    contract, menu_revision = validate_metadata(
        files["scanout_module"],
        files["scanout_metadata"],
        files["latch_rbf"],
        files["latch_metadata"],
    )
    if contract != fields["platform_contract_sha256"]:
        raise ManifestError("manifest platform contract does not match metadata")
    if menu_revision != fields["menu_revision"]:
        raise ManifestError("manifest Menu revision does not match latch metadata")


def parser() -> argparse.ArgumentParser:
    root = argparse.ArgumentParser()
    commands = root.add_subparsers(dest="command", required=True)
    create = commands.add_parser("generate")
    create.add_argument("--output", required=True, type=Path)
    create.add_argument("--main", required=True, type=Path)
    create.add_argument("--gui", required=True, type=Path)
    create.add_argument("--catalog-builder", required=True, type=Path)
    create.add_argument("--scanout-module", required=True, type=Path)
    create.add_argument("--scanout-metadata", required=True, type=Path)
    create.add_argument("--latch-rbf", required=True, type=Path)
    create.add_argument("--latch-metadata", required=True, type=Path)
    create.add_argument("--main-revision", required=True)
    create.add_argument("--magik-revision", required=True)
    create.add_argument("--artifact-root", type=Path)
    check = commands.add_parser("verify")
    check.add_argument("manifest", type=Path)
    check.add_argument("--root", type=Path)
    return root


def main() -> None:
    args = parser().parse_args()
    try:
        if args.command == "generate":
            generate(args)
        else:
            verify_manifest(args.manifest, artifact_root=args.root)
            print(f"platform manifest valid=1 path={args.manifest}")
    except ManifestError as error:
        print(f"platform manifest invalid: {error}", file=sys.stderr)
        raise SystemExit(1) from error


if __name__ == "__main__":
    main()
