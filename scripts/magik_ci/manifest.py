# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

from __future__ import annotations

import hashlib
import re
import tomllib
from pathlib import Path

from .common import atomic_write, sha256_file

SCHEMA_PATH = (
    Path(__file__).parents[2]
    / "mister"
    / "platform"
    / "contracts"
    / "platform-v3.schema.toml"
)
with SCHEMA_PATH.open("rb") as stream:
    _SCHEMA = tomllib.load(stream)
FIELDS: tuple[str, ...] = tuple(_SCHEMA["fields"])
FORMAT = str(_SCHEMA["manifest_format"])
LATCH_PROTOCOL_VERSION = str(_SCHEMA["latch_protocol_version"])
LATCH_CAPABILITY_MASK = str(_SCHEMA["latch_capability_mask"])
LAYOUTS = {name: values for name, values in _SCHEMA["layouts"].items()}


def parse_fields(
    text: str, *, repeatable_keys: frozenset[str] = frozenset()
) -> dict[str, str]:
    values: dict[str, str] = {}
    for line_number, line in enumerate(text.splitlines(), 1):
        if not line or line.startswith("#"):
            continue
        if "=" not in line:
            raise ValueError(f"invalid_platform_manifest:{line_number}")
        key, value = line.split("=", 1)
        if not key or not value or (key in values and key not in repeatable_keys):
            raise ValueError(f"invalid_platform_manifest:{line_number}")
        if key not in repeatable_keys:
            values[key] = value
    return values


def serialize(values: dict[str, str]) -> str:
    if set(values) != set(FIELDS):
        raise ValueError("invalid_platform_manifest_fields")
    return "".join(f"{field}={values[field]}\n" for field in FIELDS)


def candidate_id(values: dict[str, str]) -> str:
    digest = hashlib.sha256()
    for field in FIELDS:
        if field != "qualification_candidate_id" and field in values:
            digest.update(f"{field}={values[field]}\n".encode())
    return digest.hexdigest()


def _hex(value: str, length: int, label: str) -> None:
    if len(value) != length or not re.fullmatch(r"[0-9a-f]+", value):
        raise ValueError(f"{label}: invalid hexadecimal identity")


def verify(
    path: Path, artifact_root: Path | None = None, layout: str = "dev"
) -> dict[str, str]:
    values = parse_fields(path.read_text(encoding="utf-8"))
    if values.get("format") != FORMAT or set(values) != set(FIELDS):
        raise ValueError("unsupported_platform_manifest")
    release_number = int(values["platform_release_number"])
    if (
        release_number <= 0
        or values["platform_release"] != f"platform-v0.{release_number}"
    ):
        raise ValueError("invalid_platform_release")
    if (
        values["latch_protocol_version"] != LATCH_PROTOCOL_VERSION
        or values["latch_capability_mask"] != LATCH_CAPABILITY_MASK
    ):
        raise ValueError("unsupported_latch_protocol")
    for field in (
        "platform_bundle_id",
        "qualification_candidate_id",
        "platform_contract_sha256",
    ):
        _hex(values[field], 64, field)
    for field in ("main_revision", "magik_revision", "menu_revision"):
        _hex(values[field], 40, field)
    for field in FIELDS:
        if field.endswith("_sha256"):
            _hex(values[field], 64, field)
    if values["qualification_candidate_id"] != candidate_id(values):
        raise ValueError("qualification_candidate_id")
    if artifact_root is not None:
        paths = LAYOUTS[layout]
        for name in (
            "main",
            "gui",
            "manager",
            "scanout_module",
            "scanout_metadata",
            "latch_rbf",
            "latch_metadata",
        ):
            relative = Path(paths[name]).relative_to("/media/fat")
            artifact = artifact_root / relative
            if sha256_file(artifact) != values[f"{name}_sha256"]:
                raise ValueError(f"installed_artifact_mismatch:{name}")
    return values


def _metadata(path: Path) -> dict[str, str]:
    return parse_fields(
        path.read_text(encoding="utf-8"), repeatable_keys=frozenset({"source_status"})
    )


def generate(
    output: Path,
    artifacts: dict[str, Path],
    *,
    release_number: int,
    bundle_id: str,
    main_revision: str,
    magik_revision: str,
    layout: str = "dev",
) -> None:
    _hex(main_revision, 40, "main_revision")
    _hex(magik_revision, 40, "magik_revision")
    _hex(bundle_id, 64, "platform_bundle_id")
    for name, path in artifacts.items():
        if not path.is_file():
            raise FileNotFoundError(f"platform_artifact_missing:{name}")
    scanout = _metadata(artifacts["scanout_metadata"])
    latch = _metadata(artifacts["latch_metadata"])
    if scanout.get("module_sha256") != sha256_file(artifacts["scanout_module"]):
        raise ValueError("scanout_metadata_mismatch")
    if latch.get("rbf_sha256") != sha256_file(artifacts["latch_rbf"]):
        raise ValueError("latch_metadata_mismatch")
    if scanout.get("platform_contract_sha256") != latch.get("platform_contract_sha256"):
        raise ValueError("platform_contract_mismatch")
    values = {
        "format": FORMAT,
        "platform_release": f"platform-v0.{release_number}",
        "platform_release_number": str(release_number),
        "platform_bundle_id": bundle_id,
        "latch_protocol_version": LATCH_PROTOCOL_VERSION,
        "latch_capability_mask": LATCH_CAPABILITY_MASK,
    }
    values.update({f"{name}_path": LAYOUTS[layout][name] for name in artifacts})
    values.update(
        {f"{name}_sha256": sha256_file(path) for name, path in artifacts.items()}
    )
    values.update(
        {
            "platform_contract_sha256": scanout["platform_contract_sha256"],
            "main_revision": main_revision,
            "magik_revision": magik_revision,
            "menu_revision": latch["source_commit"],
        }
    )
    values["qualification_candidate_id"] = candidate_id(values)
    atomic_write(output, serialize(values).encode())
    verify(output, layout=layout)
