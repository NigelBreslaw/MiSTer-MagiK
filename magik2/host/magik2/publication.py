"""Publish explicit, prevalidated artifact sets through the native connection."""

from __future__ import annotations

import hashlib
import json
import subprocess
import sys
import tempfile
import uuid
from pathlib import Path

from .apps import repository
from .client import AgentError


def publish(agent, run: Path, files: dict[str, Path], **fields):
    stage = uuid.uuid4().hex
    journal = {"stage": stage, **fields, "uploaded": []}
    evidence = run / "publication.json"

    def save():
        evidence.write_text(json.dumps(journal, indent=2) + "\n")

    save()
    try:
        for artifact, path in files.items():
            body = path.read_bytes()
            sha256 = hashlib.sha256(body).hexdigest()
            header, _ = agent._request(
                "publication-upload",
                {"stage": stage, "artifact": artifact, "sha256": sha256},
                body,
                attempts=1,
                timeout=180,
            )
            if (
                header.operation != "publication-staged"
                or header.fields.get("sha256") != sha256
            ):
                raise AgentError(f"publication upload failed: {header.fields}")
            journal["uploaded"].append(
                {"artifact": artifact, "sha256": sha256, "bytes": len(body)}
            )
            save()
        header, _ = agent._request(
            "publication-commit", {"stage": stage, **fields}, attempts=1, timeout=90
        )
        journal["result"] = dict(header.fields)
        if header.operation != "publication-complete":
            raise AgentError.from_fields(header.fields)
        if header.fields.get("requires_reboot"):
            from types import SimpleNamespace
            from .device import reboot_device

            reboot_device(SimpleNamespace(attended=True), run, agent=agent)
            complete, _ = agent._request(
                "publication-control",
                {"stage": stage, "action": "finish", "attended": True},
                attempts=1,
                timeout=30,
            )
            journal["activation"] = dict(complete.fields)
            if complete.operation != "publication-complete":
                raise AgentError.from_fields(complete.fields)
        return journal
    except BaseException as error:
        journal["error"] = str(error)
        raise
    finally:
        save()


def databases(arguments, run):
    from .cli import connect_agent

    sys.path.insert(0, str(repository()))
    from scripts.magik_ci.databases import extract_release

    with tempfile.TemporaryDirectory(prefix="magik-database-stage-") as directory:
        root = Path(directory)
        extract_release(arguments.release_dir, root)
        names = (
            "magik-metadata-v1.bin",
            "arcade-updater-index-v1.lz4b",
            "game-databases-manifest.json",
        )
        files = {name: root / name for name in names}
        files["game-databases-SHA256SUMS"] = root / "SHA256SUMS"
        agent, _ = connect_agent(run, {"publication-v1"})
        print(
            json.dumps(
                publish(agent, run, files, kind="databases", layout=arguments.layout),
                indent=2,
            )
        )
    return 0


def platform(arguments, run):
    from .cli import connect_agent

    sys.path.insert(0, str(repository()))
    from scripts.magik_ci import manifest

    root = arguments.root.resolve()
    manifest_path = root / (
        Path(manifest.LAYOUTS[arguments.layout]["root"]) / "platform-v3.manifest"
    ).relative_to("/media/fat")
    # The shared contract verifier owns validation; build only that tiny package.
    subprocess.run(
        [
            str(repository() / "scripts/cargo"),
            "build",
            "--locked",
            "--manifest-path",
            str(repository() / "mister/platform/contracts/manifest/Cargo.toml"),
            "--bin",
            "platform-manifest-check",
        ],
        check=True,
        timeout=180,
    )
    # scripts/cargo shares its output across linked worktrees.
    primary = Path(
        subprocess.check_output(
            ["git", "rev-parse", "--path-format=absolute", "--git-common-dir"],
            cwd=repository(),
            text=True,
        ).strip()
    ).parent
    import os

    previous_verifier = os.environ.get("MISTER_MAGIK_MANIFEST_CHECK")
    os.environ["MISTER_MAGIK_MANIFEST_CHECK"] = str(
        primary
        / "mister/platform/contracts/manifest/target/debug/platform-manifest-check"
    )
    try:
        values = manifest.verify(
            manifest_path,
            root if arguments.kind == "platform" else None,
            layout=arguments.layout,
        )
    finally:
        if previous_verifier is None:
            os.environ.pop("MISTER_MAGIK_MANIFEST_CHECK", None)
        else:
            os.environ["MISTER_MAGIK_MANIFEST_CHECK"] = previous_verifier
    components = (
        (
            "main",
            "gui",
            "manager",
            "scanout_module",
            "scanout_metadata",
            "latch_rbf",
            "latch_metadata",
        )
        if arguments.kind == "platform"
        else (
            ("latch_rbf", "latch_metadata") if arguments.kind == "fpga" else ("main",)
        )
    )
    files = {
        name: root / Path(values[name + "_path"]).relative_to("/media/fat")
        for name in components
    }
    files["manifest"] = manifest_path
    if arguments.kind == "fpga":
        validate_fpga_signoff(
            files["latch_rbf"], files["latch_metadata"], arguments.signoff_report
        )
    if arguments.kind == "local-main":
        source = arguments.source_checkout.resolve()
        revision = subprocess.check_output(
            ["git", "rev-parse", "HEAD"], cwd=source, text=True
        ).strip()
        dirty = subprocess.check_output(
            ["git", "status", "--porcelain"], cwd=source, text=True
        ).strip()
        if dirty or revision != values["main_revision"]:
            raise ValueError(
                "local Main requires a clean committed source checkout matching the manifest"
            )
    agent, _ = connect_agent(run, {"publication-v1", "platform-publication-v1"})
    fields = dict(
        kind=arguments.kind, layout=arguments.layout, attended=arguments.attended
    )
    if arguments.kind in {"platform", "fpga"}:
        fields["activate_fpga"] = arguments.activate_fpga
    print(json.dumps(publish(agent, run, files, **fields), indent=2))
    return 0


def validate_fpga_signoff(rbf: Path, metadata: Path, report: Path):
    import math

    values = dict(
        line.split("=", 1) for line in metadata.read_text().splitlines() if "=" in line
    )
    if (
        values.get("format") != "mister-magik-fpga-release-v2"
        or values.get("quartus_mode") != "local"
        or values.get("apply_patch") != "1"
        or values.get("rbf_sha256") != hashlib.sha256(rbf.read_bytes()).hexdigest()
    ):
        raise ValueError(
            "experimental FPGA metadata does not identify the supplied local patched artifact"
        )
    summary = next(
        (
            line
            for line in report.read_text().splitlines()
            if line.startswith("quartus_delta_signoff_tsv\t")
        ),
        "",
    )
    fields = dict(item.split("=", 1) for item in summary.split("\t")[1:] if "=" in item)
    if any(
        fields.get(key) != value
        for key, value in {
            "valid": "1",
            "invalid_reason": "ok",
            "patched_tns_max_abs": "0.0",
            "custom_sync_seen": "1",
            "custom_sync_mtbf": "1",
        }.items()
    ):
        raise ValueError("FPGA signoff is incomplete or invalid")
    for name in ("patched_setup_slack_min", "patched_hold_slack_min"):
        value = float(fields.get(name, "nan"))
        if not math.isfinite(value) or value < 0.20:
            raise ValueError("FPGA signoff timing margin is below 0.20 ns")
