#!/usr/bin/env python3
"""Create and verify durable MiSTer MagiK platform bundle v0.1 archives."""

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

ROOT = Path(__file__).resolve().parents[1]
FORMAT = "mister-magik-platform-bundle-v0.1"
MANIFEST_NAME = "platform-bundle-v0.1.json"
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


def bundle_id(fpga_id: str, kernel_id: str) -> str:
    helper = load_module("platform_component_id", ROOT / "scripts/platform-component-id.py")
    try:
        return helper.bundle_id(fpga_id, kernel_id)
    except ValueError as error:
        raise BundleError(str(error)) from error


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
        verifier = load_module("verify_fpga_rbf_manifest", ROOT / "scripts/verify-fpga-rbf-manifest.py")
        try:
            fpga_fields = verifier.verify(metadata)
        except ValueError as error:
            raise BundleError(f"invalid FPGA metadata: {error}") from error
        if fpga_fields.get("component_input_sha256") != fpga_id:
            raise BundleError("FPGA component identity does not match artifact")
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
    fpga_id = args.fpga_id
    kernel_id = args.kernel_id
    require_hex("fpga_input_sha256", fpga_id, HEX64)
    require_hex("kernel_input_sha256", kernel_id, HEX64)
    for name, value in (("fpga_run_id", args.fpga_run_id), ("kernel_run_id", args.kernel_run_id)):
        if not value.isdigit():
            raise BundleError(f"invalid {name}")
    for name, value in (("fpga_head_sha", args.fpga_head_sha), ("kernel_head_sha", args.kernel_head_sha)):
        require_hex(name, value, HEX40)
    contract, _, _ = verify_component_inputs(args.fpga_dir, args.scanout_dir, fpga_id, kernel_id)
    identity = bundle_id(fpga_id, kernel_id)
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=True)
    archive = output / f"mister-magik-platform-v0.1-{identity}.zip"
    manifest = output / MANIFEST_NAME
    with tempfile.TemporaryDirectory(prefix="mister-magik-platform-bundle-") as temporary:
        stage = Path(temporary) / "bundle"
        shutil.copytree(args.fpga_dir, stage / "fpga")
        shutil.copytree(args.scanout_dir, stage / "scanout")
        payload = {
            "format": FORMAT,
            "bundle_id": identity,
            "fpga_input_sha256": fpga_id,
            "kernel_input_sha256": kernel_id,
            "platform_contract_sha256": contract,
            "components": {
                "fpga": {"workflow": "fpga-vblank-latch.yml", "run_id": args.fpga_run_id, "head_sha": args.fpga_head_sha},
                "kernel": {"workflow": "kernel-scanout.yml", "run_id": args.kernel_run_id, "head_sha": args.kernel_head_sha},
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


def verify(archive: Path, manifest_path: Path | None = None) -> dict[str, object]:
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
        stored_manifest = root / MANIFEST_NAME
        if not stored_manifest.is_file() or not (root / "SHA256SUMS").is_file():
            raise BundleError("bundle is missing its manifest or checksums")
        payload = json.loads(stored_manifest.read_text())
        if manifest_path is not None and json.loads(manifest_path.read_text()) != payload:
            raise BundleError("release manifest differs from archive manifest")
        if payload.get("format") != FORMAT:
            raise BundleError("unsupported platform bundle format")
        fpga_id = str(payload.get("fpga_input_sha256", ""))
        kernel_id = str(payload.get("kernel_input_sha256", ""))
        expected = bundle_id(fpga_id, kernel_id)
        if payload.get("bundle_id") != expected:
            raise BundleError("bundle identity does not match components")
        for component in ("fpga", "kernel"):
            origin = payload.get("components", {}).get(component, {})
            if origin.get("head_sha") is None:
                raise BundleError(f"missing {component} origin")
            require_hex(f"{component} head_sha", str(origin["head_sha"]), HEX40)
        expected_files = {entry["path"]: entry for entry in payload.get("files", [])}
        actual_files = tree_entries(root)
        actual_files = [entry for entry in actual_files if entry["path"] not in {MANIFEST_NAME, "SHA256SUMS"}]
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
        return payload


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    create_parser = commands.add_parser("create")
    create_parser.add_argument("--fpga-dir", required=True, type=Path)
    create_parser.add_argument("--scanout-dir", required=True, type=Path)
    create_parser.add_argument("--fpga-id", required=True)
    create_parser.add_argument("--kernel-id", required=True)
    create_parser.add_argument("--fpga-run-id", required=True)
    create_parser.add_argument("--kernel-run-id", required=True)
    create_parser.add_argument("--fpga-head-sha", required=True)
    create_parser.add_argument("--kernel-head-sha", required=True)
    create_parser.add_argument("--output", required=True, type=Path)
    check = commands.add_parser("verify")
    check.add_argument("archive", type=Path)
    check.add_argument("--manifest", type=Path)
    args = parser.parse_args()
    try:
        if args.command == "create":
            print(create(args))
        else:
            print(json.dumps(verify(args.archive, args.manifest), sort_keys=True))
    except (BundleError, OSError, json.JSONDecodeError, zipfile.BadZipFile) as error:
        print(f"platform bundle invalid: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
