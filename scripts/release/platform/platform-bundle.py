#!/usr/bin/env python3
# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

"""Create, plan, and verify durable numbered MiSTer MagiK platform bundles."""

from __future__ import annotations

import argparse
import hashlib
import importlib.util
import json
import shutil
import sys
import tempfile
import zipfile
from pathlib import Path, PurePosixPath

ROOT = Path(__file__).resolve().parents[3]
FORMAT_V1 = "mister-magik-platform-bundle-v0.1"
FORMAT = "mister-magik-platform-bundle-v0.2"
MANIFEST_V1 = "platform-bundle-v0.1.json"
MANIFEST_NAME = "platform-bundle-v0.2.json"
HEX40 = 40
HEX64 = 64


class BundleError(ValueError):
    pass


def digest(path: Path) -> str:
    value = hashlib.sha256()
    with path.open("rb") as source:
        for chunk in iter(lambda: source.read(1024 * 1024), b""):
            value.update(chunk)
    return value.hexdigest()


def require_hex(name: str, value: str, length: int) -> None:
    if len(value) != length or any(char not in "0123456789abcdef" for char in value):
        raise BundleError(f"invalid {name}")


def load_module(name: str, path: Path):
    spec = importlib.util.spec_from_file_location(name, path)
    assert spec and spec.loader
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def fields(path: Path) -> dict[str, str]:
    result: dict[str, str] = {}
    for number, line in enumerate(path.read_text().splitlines(), 1):
        if not line or "=" not in line:
            raise BundleError(f"malformed metadata {path}:{number}")
        key, value = line.split("=", 1)
        if not key or not value or key in result:
            raise BundleError(f"malformed metadata {path}:{number}")
        result[key] = value
    return result


def legacy_bundle_id(fpga_id: str, kernel_id: str) -> str:
    helper = load_module("platform_component_id", ROOT / "scripts/release/platform/platform-component-id.py")
    try:
        return helper.bundle_id(fpga_id, kernel_id)
    except ValueError as error:
        raise BundleError(str(error)) from error


def bundle_id(main_id: str, fpga_id: str, kernel_id: str) -> str:
    for name, value in (("main", main_id), ("fpga", fpga_id), ("kernel", kernel_id)):
        require_hex(f"{name}_input_sha256", value, HEX64)
    material = (
        f"format={FORMAT}\nmain={main_id}\nfpga={fpga_id}\nkernel={kernel_id}\n"
    )
    return hashlib.sha256(material.encode()).hexdigest()


def require_release_version(value: object) -> int:
    if isinstance(value, bool) or not isinstance(value, int) or value < 1:
        raise BundleError("invalid platform release version")
    return value


def update_plan(
    current: dict[str, object] | None,
    current_version: int,
    main_id: str,
    fpga_id: str,
    kernel_id: str,
) -> dict[str, object]:
    require_hex("main_input_sha256", main_id, HEX64)
    require_hex("fpga_input_sha256", fpga_id, HEX64)
    require_hex("kernel_input_sha256", kernel_id, HEX64)
    identity = bundle_id(main_id, fpga_id, kernel_id)
    if current is None:
        if current_version != 0:
            raise BundleError("current platform version requires a manifest")
        result = {
            "current_version": 0,
            "next_version": 1,
            "current_bundle_id": "",
            "bundle_id": identity,
            "update_needed": True,
        }
        result["release_tag"] = "platform-v0.1"
        return result
    if current_version < 1:
        raise BundleError("current platform manifest requires a positive version")
    current_format = current.get("format")
    if current_format not in (FORMAT_V1, FORMAT):
        raise BundleError("unsupported platform bundle format")
    stored_version = current.get("release_version")
    if stored_version is not None and require_release_version(stored_version) != current_version:
        raise BundleError("platform release tag and manifest version differ")
    current_fpga = str(current.get("fpga_input_sha256", ""))
    current_kernel = str(current.get("kernel_input_sha256", ""))
    if current_format == FORMAT_V1:
        current_identity = legacy_bundle_id(current_fpga, current_kernel)
    else:
        current_main = str(current.get("main_input_sha256", ""))
        current_identity = bundle_id(current_main, current_fpga, current_kernel)
    if current.get("bundle_id") != current_identity:
        raise BundleError("current bundle identity does not match components")
    result = {
        "current_version": current_version,
        "next_version": current_version + 1,
        "current_bundle_id": current_identity,
        "bundle_id": identity,
        "update_needed": current_format == FORMAT_V1 or current_identity != identity,
    }
    result["release_tag"] = f"platform-v0.{result['next_version']}"
    return result


def tree_entries(root: Path) -> list[dict[str, object]]:
    entries: list[dict[str, object]] = []
    for path in sorted(item for item in root.rglob("*") if item.is_file()):
        relative = path.relative_to(root).as_posix()
        if relative.startswith("/") or ".." in PurePosixPath(relative).parts:
            raise BundleError(f"unsafe bundle path: {relative}")
        entries.append({"path": relative, "size": path.stat().st_size, "sha256": digest(path)})
    return entries


def verify_component_inputs(fpga_root: Path, scanout_root: Path, fpga_id: str, kernel_id: str) -> tuple[str, Path, Path]:
    patched = fpga_root / "patched"
    stock = fpga_root / "stock"
    for root in (patched, stock):
        metadata = root / "menu-magik-vblank-latch.metadata.txt"
        if not metadata.is_file():
            raise BundleError(f"missing FPGA metadata: {metadata}")
        fpga_fields = fields(metadata)
        if fpga_fields.get("component_input_sha256") != fpga_id:
            raise BundleError("FPGA component identity does not match artifact")
        rbf = root / "menu-magik-vblank-latch.rbf"
        if not rbf.is_file() or fpga_fields.get("rbf_sha256") != digest(rbf):
            raise BundleError("FPGA metadata does not match RBF")
    verifier = load_module("verify_fpga_rbf_manifest", ROOT / "scripts/checks/verify-fpga-rbf-manifest.py")
    try:
        verifier.verify(patched / "menu-magik-vblank-latch.metadata.txt")
    except ValueError as error:
        raise BundleError(f"invalid patched FPGA metadata: {error}") from error
    module = scanout_root / "mister_magik_scanout_slots.ko"
    provenance = scanout_root / "provenance.txt"
    if not module.is_file() or not provenance.is_file():
        raise BundleError("missing scanout module or provenance")
    scanout_fields = fields(provenance)
    if scanout_fields.get("component_input_sha256") != kernel_id:
        raise BundleError("kernel component identity does not match artifact")
    if scanout_fields.get("module_sha256") != digest(module):
        raise BundleError("scanout provenance does not match module")
    contract = scanout_fields.get("platform_contract_sha256", "")
    require_hex("platform_contract_sha256", contract, HEX64)
    if fields(patched / "menu-magik-vblank-latch.metadata.txt").get("platform_contract_sha256") != contract:
        raise BundleError("mixed FPGA and scanout platform-contract hashes")
    return contract, patched, module


def write_checksums(root: Path, paths: list[Path]) -> None:
    (root / "SHA256SUMS").write_text(
        "".join(f"{digest(path)}  {path.relative_to(root).as_posix()}\n" for path in sorted(paths))
    )


def create(args: argparse.Namespace) -> Path:
    release_version = require_release_version(args.release_version)
    main_id = args.main_id
    fpga_id = args.fpga_id
    kernel_id = args.kernel_id
    require_hex("main_input_sha256", main_id, HEX64)
    require_hex("fpga_input_sha256", fpga_id, HEX64)
    require_hex("kernel_input_sha256", kernel_id, HEX64)
    for name, value in (("main_run_id", args.main_run_id), ("fpga_run_id", args.fpga_run_id), ("kernel_run_id", args.kernel_run_id)):
        if not value.isdigit():
            raise BundleError(f"invalid {name}")
    for name, value in (("main_head_sha", args.main_head_sha), ("fpga_head_sha", args.fpga_head_sha), ("kernel_head_sha", args.kernel_head_sha)):
        require_hex(name, value, HEX40)
    main = load_module("main_component", ROOT / "scripts/release/platform/main-component.py")
    try:
        main_receipt = main.verify(args.main_dir, args.main_head_sha)
    except (ValueError, OSError, json.JSONDecodeError) as error:
        raise BundleError(f"invalid Main component: {error}") from error
    if main_receipt.get("component_id") != main_id:
        raise BundleError("Main component identity does not match artifact")
    contract, _, _ = verify_component_inputs(args.fpga_dir, args.scanout_dir, fpga_id, kernel_id)
    identity = bundle_id(main_id, fpga_id, kernel_id)
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=True)
    archive = output / f"mister-magik-platform-v0.{release_version}.zip"
    manifest = output / MANIFEST_NAME
    with tempfile.TemporaryDirectory(prefix="mister-magik-platform-bundle-") as temporary:
        stage = Path(temporary) / "bundle"
        shutil.copytree(args.main_dir, stage / "main")
        shutil.copytree(args.fpga_dir, stage / "fpga")
        shutil.copytree(args.scanout_dir, stage / "scanout")
        payload = {
            "format": FORMAT,
            "release_version": release_version,
            "bundle_id": identity,
            "main_input_sha256": main_id,
            "fpga_input_sha256": fpga_id,
            "kernel_input_sha256": kernel_id,
            "platform_contract_sha256": contract,
            "components": {
                "main": {"workflow": "main-mister.yml", "run_id": args.main_run_id, "head_sha": args.main_head_sha, "head_branch": "mister-magik"},
                "fpga": {"workflow": "fpga-vblank-latch.yml", "run_id": args.fpga_run_id, "head_sha": args.fpga_head_sha, "head_branch": "main"},
                "kernel": {"workflow": "kernel-scanout.yml", "run_id": args.kernel_run_id, "head_sha": args.kernel_head_sha, "head_branch": "main"},
            },
            "files": tree_entries(stage),
        }
        (stage / MANIFEST_NAME).write_text(json.dumps(payload, indent=2, sort_keys=True) + "\n")
        write_checksums(stage, [path for path in stage.rglob("*") if path.is_file() and path.name != "SHA256SUMS"])
        with zipfile.ZipFile(archive, "w", compression=zipfile.ZIP_DEFLATED) as destination:
            for path in sorted(item for item in stage.rglob("*") if item.is_file()):
                destination.write(path, path.relative_to(stage).as_posix())
        shutil.copyfile(stage / MANIFEST_NAME, manifest)
        shutil.copyfile(stage / "SHA256SUMS", output / "SHA256SUMS")
    verify(archive, manifest)
    return archive


def verify(
    archive: Path,
    manifest_path: Path | None = None,
    expected_release_version: int | None = None,
) -> dict[str, object]:
    if not archive.is_file():
        raise BundleError(f"missing bundle archive: {archive}")
    with tempfile.TemporaryDirectory(prefix="mister-magik-platform-verify-") as temporary:
        root = Path(temporary)
        with zipfile.ZipFile(archive) as source:
            for info in source.infolist():
                path = PurePosixPath(info.filename)
                if path.is_absolute() or ".." in path.parts or not info.filename or info.is_dir():
                    raise BundleError(f"unsafe archive member: {info.filename}")
                source.extract(info, root)
        manifest_name = MANIFEST_NAME if (root / MANIFEST_NAME).is_file() else MANIFEST_V1
        stored_manifest = root / manifest_name
        if not stored_manifest.is_file() or not (root / "SHA256SUMS").is_file():
            raise BundleError("bundle is missing its manifest or checksums")
        payload = json.loads(stored_manifest.read_text())
        if manifest_path is not None and json.loads(manifest_path.read_text()) != payload:
            raise BundleError("release manifest differs from archive manifest")
        bundle_format = payload.get("format")
        if bundle_format not in (FORMAT_V1, FORMAT):
            raise BundleError("unsupported platform bundle format")
        release_version = payload.get("release_version")
        if release_version is not None:
            require_release_version(release_version)
        if expected_release_version is not None:
            expected_release_version = require_release_version(expected_release_version)
            if release_version is None and expected_release_version != 1:
                raise BundleError("numbered platform release is missing its version")
            if release_version is not None and release_version != expected_release_version:
                raise BundleError("platform release tag and manifest version differ")
        fpga_id = str(payload.get("fpga_input_sha256", ""))
        kernel_id = str(payload.get("kernel_input_sha256", ""))
        main_id = str(payload.get("main_input_sha256", ""))
        expected = legacy_bundle_id(fpga_id, kernel_id) if bundle_format == FORMAT_V1 else bundle_id(main_id, fpga_id, kernel_id)
        if payload.get("bundle_id") != expected:
            raise BundleError("bundle identity does not match components")
        components = (("fpga", "kernel") if bundle_format == FORMAT_V1 else ("main", "fpga", "kernel"))
        workflows = {
            "main": "main-mister.yml",
            "fpga": "fpga-vblank-latch.yml",
            "kernel": "kernel-scanout.yml",
        }
        for component in components:
            origin = payload.get("components", {}).get(component, {})
            if origin.get("head_sha") is None:
                raise BundleError(f"missing {component} origin")
            require_hex(f"{component} head_sha", str(origin["head_sha"]), HEX40)
            expected_branch = "mister-magik" if component == "main" else "main"
            if origin.get("head_branch") != expected_branch:
                raise BundleError(f"{component} origin is not {expected_branch}")
            if origin.get("workflow") != workflows[component]:
                raise BundleError(f"invalid {component} origin workflow")
            run_id = origin.get("run_id")
            if not isinstance(run_id, str) or not run_id.isdigit() or int(run_id) < 1:
                raise BundleError(f"invalid {component} origin run ID")
        expected_files = {entry["path"]: entry for entry in payload.get("files", [])}
        actual_files = tree_entries(root)
        actual_files = [entry for entry in actual_files if entry["path"] not in {manifest_name, "SHA256SUMS"}]
        if expected_files != {entry["path"]: entry for entry in actual_files}:
            raise BundleError("bundle file manifest does not match archive")
        checksums = fields(root / "SHA256SUMS") if False else None
        for line in (root / "SHA256SUMS").read_text().splitlines():
            value, relative = line.split("  ", 1)
            path = root / relative
            if not path.is_file() or digest(path) != value:
                raise BundleError("bundle checksum mismatch")
        contract, _, _ = verify_component_inputs(root / "fpga", root / "scanout", fpga_id, kernel_id)
        if contract != payload.get("platform_contract_sha256"):
            raise BundleError("bundle platform contract does not match components")
        if bundle_format == FORMAT:
            main = load_module("main_component_verify", ROOT / "scripts/release/platform/main-component.py")
            try:
                receipt = main.verify(root / "main", str(payload["components"]["main"]["head_sha"]))
            except (ValueError, OSError, json.JSONDecodeError) as error:
                raise BundleError(f"invalid bundled Main component: {error}") from error
            if receipt.get("component_id") != main_id:
                raise BundleError("bundled Main identity does not match receipt")
        return payload


def extract_component(
    archive: Path,
    manifest_path: Path,
    component: str,
    component_id: str,
    output: Path,
) -> dict[str, object]:
    require_hex(f"{component}_input_sha256", component_id, HEX64)
    payload = verify(archive, manifest_path)
    keys = {"main": "main_input_sha256", "fpga": "fpga_input_sha256", "kernel": "kernel_input_sha256"}
    directories = {"main": "main", "fpga": "fpga", "kernel": "scanout"}
    if component not in keys:
        raise BundleError(f"unsupported component: {component}")
    if payload.get(keys[component]) is None:
        raise BundleError(f"platform bundle does not contain {component}")
    if payload.get(keys[component]) != component_id:
        raise BundleError(f"platform bundle {component} identity does not match request")
    origin = payload.get("components", {}).get(component)
    if not isinstance(origin, dict):
        raise BundleError(f"platform bundle is missing {component} origin")
    if output.exists():
        raise BundleError(f"component output already exists: {output}")
    with tempfile.TemporaryDirectory(prefix="mister-magik-platform-extract-") as temporary:
        root = Path(temporary)
        with zipfile.ZipFile(archive) as source:
            source.extractall(root)
        shutil.copytree(root / directories[component], output)
    return {
        "component": component,
        "component_id": component_id,
        "run_id": origin["run_id"],
        "head_sha": origin["head_sha"],
        "workflow": origin["workflow"],
        "head_branch": origin["head_branch"],
        "release_version": payload.get("release_version", 1),
    }


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    create_parser = commands.add_parser("create")
    create_parser.add_argument("--main-dir", required=True, type=Path)
    create_parser.add_argument("--fpga-dir", required=True, type=Path)
    create_parser.add_argument("--scanout-dir", required=True, type=Path)
    create_parser.add_argument("--main-id", required=True)
    create_parser.add_argument("--fpga-id", required=True)
    create_parser.add_argument("--kernel-id", required=True)
    create_parser.add_argument("--main-run-id", required=True)
    create_parser.add_argument("--fpga-run-id", required=True)
    create_parser.add_argument("--kernel-run-id", required=True)
    create_parser.add_argument("--main-head-sha", required=True)
    create_parser.add_argument("--fpga-head-sha", required=True)
    create_parser.add_argument("--kernel-head-sha", required=True)
    create_parser.add_argument("--release-version", required=True, type=int)
    create_parser.add_argument("--output", required=True, type=Path)
    check = commands.add_parser("verify")
    check.add_argument("archive", type=Path)
    check.add_argument("--manifest", type=Path)
    check.add_argument("--release-version", type=int)
    extract = commands.add_parser("extract-component")
    extract.add_argument("archive", type=Path)
    extract.add_argument("--manifest", required=True, type=Path)
    extract.add_argument("--component", required=True, choices=("main", "fpga", "kernel"))
    extract.add_argument("--component-id", required=True)
    extract.add_argument("--output", required=True, type=Path)
    plan = commands.add_parser("plan-update")
    plan.add_argument("--manifest", type=Path)
    plan.add_argument("--current-version", required=True, type=int)
    plan.add_argument("--main-id", required=True)
    plan.add_argument("--fpga-id", required=True)
    plan.add_argument("--kernel-id", required=True)
    plan.add_argument("--github-output", type=Path)
    args = parser.parse_args()
    try:
        if args.command == "create":
            print(create(args))
        elif args.command == "verify":
            print(json.dumps(verify(args.archive, args.manifest, args.release_version), sort_keys=True))
        elif args.command == "extract-component":
            print(json.dumps(extract_component(args.archive, args.manifest, args.component, args.component_id, args.output), sort_keys=True))
        else:
            current = json.loads(args.manifest.read_text()) if args.manifest else None
            result = update_plan(current, args.current_version, args.main_id, args.fpga_id, args.kernel_id)
            if args.github_output:
                with args.github_output.open("a") as output:
                    for key, value in result.items():
                        rendered = str(value).lower() if isinstance(value, bool) else str(value)
                        output.write(f"{key}={rendered}\n")
            print(json.dumps(result, sort_keys=True))
    except (BundleError, OSError, json.JSONDecodeError, zipfile.BadZipFile) as error:
        print(f"platform bundle invalid: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
