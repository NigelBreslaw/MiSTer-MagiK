#!/usr/bin/env python3
# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

"""Verify a latch RBF and every report against its adjacent release metadata."""

from __future__ import annotations

import argparse
import hashlib
import re
import sys
from pathlib import Path

SHA256_RE = re.compile(r"^[0-9a-f]{64}$")
COMMIT_RE = re.compile(r"^[0-9a-f]{40}$")
STALE_RE = re.compile(r"mailbox|axi|acp|descriptor|ownership|fence", re.IGNORECASE)
ANALYSIS_CONSTRAINT_OVERRIDE = "clock_groups_exclusive_to_asynchronous"
LEGACY_ANALYSIS_CONSTRAINT_SOURCE_STATUSES = (" M sys/sys_top.sdc",)
CURRENT_ANALYSIS_CONSTRAINT_SOURCE_STATUSES = (
    " M menu.qsf",
    " M sys/sys_top.sdc",
)
LEGACY_SCHEMA14_RBF_SHA256 = (
    "ef1920500c925d35b23808792f0930954446a6030b33d3e92c0f4feccd23106e"
)
DIAGNOSTIC_ARCHITECTURES = {
    "scaler-fetch-liveness-first-stall-v1",
    "scaler-fetch-no-request-gates-v1",
    "scaler-output-scheduler-gates-v1",
    "scaler-pre-read-scheduler-evidence-v1",
    "scaler-off-domain-scheduler-snapshot-v1",
    "scaler-off-domain-scheduler-snapshot-v2",
    "stock-uninstrumented-v1",
}
CANONICAL_QUARTUS_SEED_SOURCE = (
    Path(__file__).resolve().parents[2]
    / "mister/platform/fpga/menu-vblank-latch/Quartus.seed"
).read_bytes()
if not CANONICAL_QUARTUS_SEED_SOURCE.endswith(b"\n"):
    raise ValueError("canonical Quartus seed file must end with one newline")
CANONICAL_QUARTUS_SEED_BYTES = CANONICAL_QUARTUS_SEED_SOURCE.removesuffix(b"\n")
if not re.fullmatch(rb"[1-9][0-9]*", CANONICAL_QUARTUS_SEED_BYTES):
    raise ValueError("canonical Quartus seed must be a positive integer")
CANONICAL_QUARTUS_SEED = CANONICAL_QUARTUS_SEED_BYTES.decode("ascii")


def digest(path: Path) -> str:
    hasher = hashlib.sha256()
    with path.open("rb") as source:
        for chunk in iter(lambda: source.read(1024 * 1024), b""):
            hasher.update(chunk)
    return hasher.hexdigest()


def verify(
    metadata_path: Path,
    *,
    require_protocol: bool = True,
    historical_v2: bool = False,
) -> dict[str, str]:
    if not metadata_path.is_file():
        raise ValueError(f"missing metadata: {metadata_path}")
    fields: dict[str, str] = {}
    source_statuses: list[str] = []
    for number, raw in enumerate(metadata_path.read_text().splitlines(), 1):
        if not raw or "=" not in raw:
            raise ValueError(f"invalid metadata line {number}")
        key, value = raw.split("=", 1)
        if key == "source_status":
            if STALE_RE.search(value):
                raise ValueError(f"stale mailbox-era metadata: {key}")
            source_statuses.append(value)
            continue
        if key in fields:
            raise ValueError(f"duplicate metadata key: {key}")
        if STALE_RE.search(key) or STALE_RE.search(value):
            raise ValueError(f"stale mailbox-era metadata: {key}")
        fields[key] = value

    required = {
        "format",
        "platform_contract_sha256",
        "magik_commit",
        "builder_commit",
        "source_commit",
        "patch_sha256",
        "latch_rtl_sha256",
        "quartus_seed",
        "quartus_version",
        "workflow_url",
        "rbf_file",
        "rbf_sha256",
        "signoff_valid",
        "build_date",
    }
    if require_protocol:
        required.update(("latch_protocol_sha256", "latch_protocol_version"))
    if not historical_v2:
        required.update(
            (
                "latch_bridge_sha256",
                "latch_capability_mask",
                "component_input_sha256",
                "component_revision",
            )
        )
        if (
            fields.get("rbf_sha256") != LEGACY_SCHEMA14_RBF_SHA256
            and fields.get("apply_patch") != "0"
        ):
            required.add("diagnostic_architecture")
    missing = sorted(required - fields.keys())
    if missing:
        raise ValueError("missing metadata fields: " + ", ".join(missing))
    expected_format = (
        "mister-magik-fpga-release-v1"
        if historical_v2
        else "mister-magik-fpga-release-v2"
    )
    if fields["format"] != expected_format:
        raise ValueError(f"release metadata must use {expected_format}")
    for name in ("magik_commit", "builder_commit", "source_commit"):
        if not COMMIT_RE.fullmatch(fields[name]):
            raise ValueError(f"{name} must be a full commit SHA")
    for name in (
        "platform_contract_sha256",
        "patch_sha256",
        "latch_rtl_sha256",
        "rbf_sha256",
    ):
        if not SHA256_RE.fullmatch(fields[name]):
            raise ValueError(f"invalid SHA-256 in {name}")
    architecture = fields.get("diagnostic_architecture")
    if architecture is not None and architecture not in DIAGNOSTIC_ARCHITECTURES:
        raise ValueError(f"unsupported diagnostic architecture: {architecture}")
    if "latch_protocol_sha256" in fields and not SHA256_RE.fullmatch(
        fields["latch_protocol_sha256"]
    ):
        raise ValueError("invalid SHA-256 in latch_protocol_sha256")
    if "latch_bridge_sha256" in fields and not SHA256_RE.fullmatch(
        fields["latch_bridge_sha256"]
    ):
        raise ValueError("invalid SHA-256 in latch_bridge_sha256")
    if "component_input_sha256" in fields and not SHA256_RE.fullmatch(
        fields["component_input_sha256"]
    ):
        raise ValueError("invalid component_input_sha256")
    if "component_revision" in fields and not COMMIT_RE.fullmatch(
        fields["component_revision"]
    ):
        raise ValueError("component_revision must be a full commit SHA")
    if "magik_status" in fields:
        raise ValueError("release source tree was dirty")
    analysis_override = fields.get("analysis_constraint_override")
    expected_source_statuses = (
        LEGACY_ANALYSIS_CONSTRAINT_SOURCE_STATUSES
        if historical_v2
        else CURRENT_ANALYSIS_CONSTRAINT_SOURCE_STATUSES
    )
    if not source_statuses:
        if analysis_override is not None:
            raise ValueError("analysis constraint override lacks source evidence")
    elif (
        tuple(source_statuses) != expected_source_statuses
        or analysis_override != ANALYSIS_CONSTRAINT_OVERRIDE
    ):
        raise ValueError("release source tree was dirty")
    expected_seed = "1" if historical_v2 else CANONICAL_QUARTUS_SEED
    if fields["quartus_seed"] != expected_seed:
        raise ValueError(f"release seed must be {expected_seed}")
    expected_protocol = "2" if historical_v2 else "5"
    if (
        "latch_protocol_version" in fields
        and fields["latch_protocol_version"] != expected_protocol
    ):
        raise ValueError(
            f"release must bind latch protocol version {expected_protocol}"
        )
    if not historical_v2 and fields["latch_capability_mask"] != "0x03ff":
        raise ValueError("release must bind latch capability mask 0x03ff")
    if not fields["quartus_version"].startswith("17.0"):
        raise ValueError("release must use Quartus 17.0")
    if not fields["workflow_url"].startswith("https://"):
        raise ValueError("release workflow URL is not immutable evidence")
    if fields["signoff_valid"] != "1":
        raise ValueError("Quartus custom-delta signoff is not valid")
    if not re.fullmatch(r"[0-9]{6}", fields["build_date"]):
        raise ValueError("build_date must be a pinned YYMMDD value")

    root = metadata_path.parent
    rbf = root / fields["rbf_file"]
    if not rbf.is_file() or digest(rbf) != fields["rbf_sha256"]:
        raise ValueError("RBF hash mismatch")
    reports = {
        key.removeprefix("report_sha256."): value
        for key, value in fields.items()
        if key.startswith("report_sha256.")
    }
    if not reports:
        raise ValueError("manifest contains no Quartus reports")
    if "reports/quartus-delta-signoff.tsv" not in reports:
        raise ValueError("manifest does not bind the Quartus delta-signoff report")
    for relative, expected in reports.items():
        if not SHA256_RE.fullmatch(expected):
            raise ValueError(f"invalid report SHA-256: {relative}")
        report = root / relative
        if not report.is_file() or digest(report) != expected:
            raise ValueError(f"report hash mismatch: {relative}")
    return fields


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("metadata", type=Path)
    parser.add_argument(
        "--historical-v2",
        action="store_true",
        help="verify an immutable protocol-v2 rollback artifact; never use for a new build",
    )
    args = parser.parse_args()
    try:
        fields = verify(args.metadata.resolve(), historical_v2=args.historical_v2)
    except (OSError, ValueError) as error:
        print(f"FPGA manifest verification failed: {error}", file=sys.stderr)
        return 1
    print(f"FPGA manifest valid=1 rbf_sha256={fields['rbf_sha256']}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
