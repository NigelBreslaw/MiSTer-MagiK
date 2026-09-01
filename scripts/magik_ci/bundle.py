# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

from __future__ import annotations

import json
import os
import shutil
import zipfile
from pathlib import Path
from typing import Any, cast

from .common import atomic_write, sha256_bytes, sha256_file

FORMAT = "mister-magik-platform-bundle-v0.2"
MANIFEST = "platform-bundle-v0.2.json"
ORIGIN = "platform-component-origin-v1.json"
CHECKSUMS = "platform-component-SHA256SUMS"
ASSEMBLY_REVISION = 1
PATCHED_DIAGNOSTIC_ARCHITECTURE = "scaler-off-domain-scheduler-terminal-v6"
HISTORICAL_DIAGNOSTIC_ARCHITECTURES = frozenset(
    {
        "scaler-fetch-no-request-gates-v1",
        "scaler-output-scheduler-gates-v1",
        "scaler-pre-read-scheduler-evidence-v1",
        "scaler-off-domain-scheduler-snapshot-v1",
        "scaler-off-domain-scheduler-snapshot-v2",
        "scaler-off-domain-scheduler-terminal-v3",
        "scaler-off-domain-scheduler-terminal-v4",
        PATCHED_DIAGNOSTIC_ARCHITECTURE,
    }
)


def bundle_id(
    main: str,
    fpga: str,
    kernel: str,
    assembly_revision: int = ASSEMBLY_REVISION,
) -> str:
    for value in (main, fpga, kernel):
        if len(value) != 64 or any(char not in "0123456789abcdef" for char in value):
            raise ValueError("invalid component identity")
    if assembly_revision not in (0, ASSEMBLY_REVISION):
        raise ValueError("unsupported platform assembly revision")
    revision = f"assembly_revision={assembly_revision}\n" if assembly_revision else ""
    return sha256_bytes(
        f"format={FORMAT}\nmain={main}\nfpga={fpga}\nkernel={kernel}\n{revision}".encode()
    )


def update_plan(
    current: dict[str, object] | None,
    current_version: int,
    main: str,
    fpga: str,
    kernel: str,
) -> dict[str, object]:
    identity = bundle_id(main, fpga, kernel)
    next_version = current_version + 1
    if current is None:
        if current_version:
            raise ValueError("platform_manifest_missing")
        old = {
            "main_input_sha256": "",
            "fpga_input_sha256": "",
            "kernel_input_sha256": "",
            "bundle_id": "",
        }
    else:
        old = current
        old_revision = old.get("assembly_revision", 0)
        if not isinstance(old_revision, int) or isinstance(old_revision, bool):
            raise TypeError("platform_assembly_revision")
        if old.get("bundle_id") != bundle_id(
            str(old["main_input_sha256"]),
            str(old["fpga_input_sha256"]),
            str(old["kernel_input_sha256"]),
            old_revision,
        ):
            raise ValueError("platform_bundle_identity")
    return {
        "current_version": current_version,
        "next_version": next_version,
        "current_bundle_id": old["bundle_id"],
        "bundle_id": identity,
        "update_needed": old["bundle_id"] != identity,
        "main_changed": old["main_input_sha256"] != main,
        "fpga_changed": old["fpga_input_sha256"] != fpga,
        "kernel_changed": old["kernel_input_sha256"] != kernel,
        "release_tag": f"platform-v0.{next_version}",
    }


def _files(root: Path) -> list[tuple[str, bytes]]:
    return [
        (str(path.relative_to(root)).replace(os.sep, "/"), path.read_bytes())
        for path in sorted(root.rglob("*"))
        if path.is_file()
    ]


def _zip_write(path: Path, files: list[tuple[str, bytes]]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    with zipfile.ZipFile(
        path, "w", compression=zipfile.ZIP_DEFLATED, compresslevel=9
    ) as archive:
        for name, data in files:
            info = zipfile.ZipInfo(name, date_time=(1980, 1, 1, 0, 0, 0))
            info.compress_type = zipfile.ZIP_DEFLATED
            info.external_attr = 0o644 << 16
            archive.writestr(info, data)


def create(
    *,
    main: Path,
    fpga: Path,
    scanout: Path,
    main_id: str,
    fpga_id: str,
    kernel_id: str,
    main_run_id: str,
    fpga_run_id: str,
    kernel_run_id: str,
    main_head_sha: str,
    fpga_head_sha: str,
    kernel_head_sha: str,
    main_source: str,
    fpga_source: str,
    kernel_source: str,
    release_version: int,
    output: Path,
) -> Path:
    identity = bundle_id(main_id, fpga_id, kernel_id)
    entries: list[tuple[str, bytes]] = []
    for prefix, root in (("main", main), ("fpga", fpga), ("scanout", scanout)):
        entries.extend((f"{prefix}/{name}", data) for name, data in _files(root))
    entries.sort()
    payload = {
        "format": FORMAT,
        "assembly_revision": ASSEMBLY_REVISION,
        "release_version": release_version,
        "bundle_id": identity,
        "main_input_sha256": main_id,
        "fpga_input_sha256": fpga_id,
        "kernel_input_sha256": kernel_id,
        "platform_contract_sha256": _metadata_value(
            fpga / "patched/menu-magik-vblank-latch.metadata.txt",
            "platform_contract_sha256",
        ),
        "latch_rbf_sha256": _metadata_value(
            fpga / "patched/menu-magik-vblank-latch.metadata.txt", "rbf_sha256"
        ),
        "latch_protocol_sha256": _metadata_value(
            fpga / "patched/menu-magik-vblank-latch.metadata.txt",
            "latch_protocol_sha256",
        ),
        "latch_protocol_version": int(
            _metadata_value(
                fpga / "patched/menu-magik-vblank-latch.metadata.txt",
                "latch_protocol_version",
            )
        ),
        "diagnostic_architecture": _metadata_value(
            fpga / "patched/menu-magik-vblank-latch.metadata.txt",
            "diagnostic_architecture",
        ),
        "components": {
            "main": {
                "component": "main",
                "run_id": main_run_id,
                "head_sha": main_head_sha,
                "head_branch": "mister-magik",
                "source": main_source,
            },
            "fpga": {
                "component": "fpga",
                "run_id": fpga_run_id,
                "head_sha": fpga_head_sha,
                "head_branch": "main",
                "source": fpga_source,
            },
            "kernel": {
                "component": "kernel",
                "run_id": kernel_run_id,
                "head_sha": kernel_head_sha,
                "head_branch": "main",
                "source": kernel_source,
            },
        },
        "files": [
            {"path": name, "size": len(data), "sha256": sha256_bytes(data)}
            for name, data in entries
        ],
    }
    manifest = (json.dumps(payload, indent=2, sort_keys=True) + "\n").encode()
    checksums = (
        "".join(f"{sha256_bytes(data)}  {name}\n" for name, data in entries)
        + f"{sha256_bytes(manifest)}  {MANIFEST}\n"
    )
    archive = output / f"mister-magik-platform-v0.{release_version}.zip"
    _zip_write(
        archive, entries + [(MANIFEST, manifest), ("SHA256SUMS", checksums.encode())]
    )
    atomic_write(output / MANIFEST, manifest)
    atomic_write(output / "SHA256SUMS", checksums.encode())
    verify(archive, output / MANIFEST, release_version)
    return archive


def _metadata_value(path: Path, key: str) -> str:
    for line in path.read_text(encoding="utf-8").splitlines():
        if line.startswith(f"{key}="):
            return line.split("=", 1)[1]
    return ""


def _validate_diagnostic_architecture(
    embedded: str, declared: object, *, historical_baseline: bool
) -> None:
    allowed = (
        HISTORICAL_DIAGNOSTIC_ARCHITECTURES
        if historical_baseline
        else frozenset({PATCHED_DIAGNOSTIC_ARCHITECTURE})
    )
    if embedded not in allowed or declared != embedded:
        raise ValueError("fpga_diagnostic_architecture")


def verify(
    archive: Path,
    manifest: Path | None = None,
    release_version: int | None = None,
    *,
    historical_baseline: bool = False,
) -> dict[str, object]:
    with zipfile.ZipFile(archive) as stream:
        files = {name: stream.read(name) for name in stream.namelist()}
    payload = cast(dict[str, Any], json.loads(files[MANIFEST]))
    if payload.get("format") != FORMAT or (
        release_version is not None
        and payload.get("release_version") != release_version
    ):
        raise ValueError("platform_bundle_manifest")
    assembly_revision = payload.get("assembly_revision", 0)
    if not isinstance(assembly_revision, int) or isinstance(assembly_revision, bool):
        raise TypeError("platform_assembly_revision")
    expected = bundle_id(
        str(payload["main_input_sha256"]),
        str(payload["fpga_input_sha256"]),
        str(payload["kernel_input_sha256"]),
        assembly_revision,
    )
    if payload.get("bundle_id") != expected:
        raise ValueError("platform_bundle_identity")
    actual = {
        name: (len(data), sha256_bytes(data))
        for name, data in files.items()
        if name not in (MANIFEST, "SHA256SUMS")
    }
    declared = {
        entry["path"]: (entry["size"], entry["sha256"])
        for entry in payload.get("files", [])
    }
    if actual != declared:
        raise ValueError("platform_file_manifest")
    if assembly_revision >= 1:
        metadata = files.get(
            "fpga/patched/menu-magik-vblank-latch.metadata.txt", b""
        ).decode()
        embedded_rbf = _metadata_text_value(metadata, "rbf_sha256")
        embedded_contract = _metadata_text_value(metadata, "platform_contract_sha256")
        embedded_protocol = _metadata_text_value(metadata, "latch_protocol_sha256")
        embedded_protocol_version = _metadata_text_value(
            metadata, "latch_protocol_version"
        )
        embedded_architecture = _metadata_text_value(
            metadata, "diagnostic_architecture"
        )
        if payload.get("platform_contract_sha256") != embedded_contract:
            raise ValueError("platform_contract_mismatch")
        if payload.get("latch_rbf_sha256") != embedded_rbf:
            raise ValueError("fpga_rbf_identity")
        if (
            payload.get("latch_protocol_sha256") != embedded_protocol
            or str(payload.get("latch_protocol_version", ""))
            != embedded_protocol_version
        ):
            raise ValueError("latch_protocol_identity")
        _validate_diagnostic_architecture(
            embedded_architecture,
            payload.get("diagnostic_architecture"),
            historical_baseline=historical_baseline,
        )
    for line in files.get("SHA256SUMS", b"").decode().splitlines():
        digest, name = line.split("  ", 1)
        if name not in files or sha256_bytes(files[name]) != digest:
            raise ValueError(f"platform_checksum:{name}")
    if manifest is not None and manifest.read_bytes() != files[MANIFEST]:
        raise ValueError("platform_release_manifest_mismatch")
    return payload


def _metadata_text_value(text: str, key: str) -> str:
    for line in text.splitlines():
        if line.startswith(f"{key}="):
            return line.split("=", 1)[1]
    return ""


def extract_component(
    archive: Path,
    manifest: Path,
    component: str,
    component_id: str,
    output: Path,
    *,
    historical_baseline: bool = False,
) -> dict[str, object]:
    payload = cast(
        dict[str, Any],
        verify(archive, manifest, historical_baseline=historical_baseline),
    )
    expected = {
        "main": "main_input_sha256",
        "fpga": "fpga_input_sha256",
        "kernel": "kernel_input_sha256",
    }[component]
    if payload[expected] != component_id:
        raise ValueError("component_identity_mismatch")
    if output.exists():
        raise FileExistsError(output)
    output.mkdir(parents=True)
    prefix = "scanout/" if component == "kernel" else f"{component}/"
    with zipfile.ZipFile(archive) as stream:
        for name in stream.namelist():
            if name.startswith(prefix):
                destination = output / name[len(prefix) :]
                destination.parent.mkdir(parents=True, exist_ok=True)
                destination.write_bytes(stream.read(name))
    components = cast(dict[str, dict[str, object]], payload["components"])
    origin = components[component]
    return {
        "component": component,
        "component_id": component_id,
        **origin,
        "release_version": payload["release_version"],
    }


def write_component_cache(
    component: str, artifact: Path, component_id: str, run_id: str, head_sha: str
) -> None:
    origin = {
        "format": "mister-magik-platform-component-origin-v1",
        "component": component,
        "component_id": component_id,
        "workflow": "platform-bundle.yml",
        "run_id": run_id,
        "head_sha": head_sha,
        "head_branch": "mister-magik" if component == "main" else "main",
    }
    atomic_write(
        artifact / ORIGIN,
        (json.dumps(origin, indent=2, sort_keys=True) + "\n").encode(),
    )
    lines = []
    for path in sorted(artifact.rglob("*")):
        if (
            path.is_file()
            and path.name not in (ORIGIN, CHECKSUMS)
            and not any(
                part.startswith(".") for part in path.relative_to(artifact).parts
            )
        ):
            lines.append(
                f"{sha256_file(path)}  {path.relative_to(artifact).as_posix()}\n"
            )
    atomic_write(artifact / CHECKSUMS, "".join(lines).encode())


def verify_component(
    component: str, artifact: Path, component_id: str, revision: str | None = None
) -> dict[str, object]:
    origin = json.loads((artifact / ORIGIN).read_text(encoding="utf-8"))
    if (
        origin.get("component") != component
        or origin.get("component_id") != component_id
    ):
        raise ValueError("component_origin")
    for line in (artifact / CHECKSUMS).read_text(encoding="utf-8").splitlines():
        digest, name = line.split("  ", 1)
        if sha256_file(artifact / name) != digest:
            raise ValueError(f"component_checksum:{name}")
    return {"component": component, "component_id": component_id, "origin": origin}


def compact_component(
    component: str, artifact: Path, output: Path, component_id: str
) -> Path:
    verify_component(component, artifact, component_id)
    if component != "fpga":
        raise ValueError("component_compaction_unsupported")
    shutil.copytree(artifact, output)
    return output
