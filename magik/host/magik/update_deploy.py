"""Apply a pinned support-release pair before ordinary application deployment."""

from __future__ import annotations

from contextlib import nullcontext
import hashlib
import json
import os
from pathlib import Path
import subprocess
import tempfile
import zipfile

from .apps import repository
from .build import ensure_arm_application, ensure_arm_package
from .client import AgentError
from .publication import publish
from .updates import atomic_json, contracts, lock, names, root, verify

PLATFORM_MEMBERS = {
    "main": "main/MiSTer_MagiK",
    "scanout_module": "scanout/mister_magik_scanout_slots.ko",
    "scanout_metadata": "scanout/provenance.txt",
    "latch_rbf": "fpga/patched/menu-magik-vblank-latch.rbf",
    "latch_metadata": "fpga/patched/menu-magik-vblank-latch.metadata.txt",
}


def state(agent):
    reply, _ = agent._request(
        "publication-state", {"layout": "dev"}, attempts=2, timeout=60
    )
    if reply.operation != "publication-state":
        raise AgentError.from_fields(reply.fields)
    return dict(reply.fields)


def extract(archive, destination):
    with zipfile.ZipFile(archive) as stream:
        for member in stream.infolist():
            path = destination / member.filename
            if not path.resolve().is_relative_to(destination.resolve()):
                raise ValueError("unsafe release archive path")
        stream.extractall(destination)


def unique(directory, pattern):
    paths = list(directory.rglob(pattern))
    if len(paths) != 1:
        raise ValueError(f"expected one {pattern} in {directory}")
    return paths[0]


def prepare(pair, directory):
    contracts()
    from scripts.magik_ci import manifest

    platform = pair["platform"]
    payload = verify("platform", platform)
    extracted = directory / "platform"
    extract(
        Path(platform["directory"]) / names("platform", platform["version"])[0],
        extracted,
    )
    package = repository() / "apps/mister"
    gui = ensure_arm_application(package, package / "target/magik-build.json").artifact
    package = repository() / "mister/tools/manager"
    manager = ensure_arm_package(package, package / "target/magik-build.json").artifact
    files = {
        "main": unique(extracted / "main", "MiSTer_MagiK"),
        "gui": gui,
        "manager": manager,
        "scanout_module": unique(extracted / "scanout", "*.ko"),
        "scanout_metadata": unique(extracted / "scanout", "provenance.txt"),
        "latch_rbf": extracted / "fpga/patched/menu-magik-vblank-latch.rbf",
        "latch_metadata": extracted
        / "fpga/patched/menu-magik-vblank-latch.metadata.txt",
    }
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
    )
    manifest_path = directory / "platform-v3.manifest"
    receipt = json.loads(
        unique(extracted / "main", "main-component-v0.1.json").read_text()
    )
    revision = subprocess.check_output(
        ["git", "rev-parse", "HEAD"], cwd=repository(), text=True
    ).strip()
    primary = Path(
        subprocess.check_output(
            ["git", "rev-parse", "--path-format=absolute", "--git-common-dir"],
            cwd=repository(),
            text=True,
        ).strip()
    ).parent
    previous = os.environ.get("MISTER_MAGIK_MANIFEST_CHECK")
    os.environ["MISTER_MAGIK_MANIFEST_CHECK"] = str(
        primary
        / "mister/platform/contracts/manifest/target/debug/platform-manifest-check"
    )
    try:
        manifest.generate(
            manifest_path,
            files,
            release_number=platform["version"],
            bundle_id=payload["bundle_id"],
            main_revision=receipt["source_revision"],
            magik_revision=revision,
            layout="dev",
        )
    finally:
        if previous is None:
            os.environ.pop("MISTER_MAGIK_MANIFEST_CHECK", None)
        else:
            os.environ["MISTER_MAGIK_MANIFEST_CHECK"] = previous
    files["manifest"] = manifest_path
    return files


def database_files(pair, directory):
    _, databases = contracts()
    databases.extract_release(Path(pair["databases"]["directory"]), directory)
    files = {
        name: directory / name
        for name in (
            "magik-metadata-v1.bin",
            "arcade-updater-index-v1.lz4b",
            "game-databases-manifest.json",
        )
    }
    files["game-databases-SHA256SUMS"] = directory / "SHA256SUMS"
    return files


def needed(installed, release, kind, payload):
    version = int(installed.get("version") or 0)
    if version > release["version"]:
        if not installed.get("verified"):
            raise AgentError(f"newer installed {kind} is invalid; refusing downgrade")
        return False
    matches = version == release["version"] and installed.get("verified")
    if kind == "platform":
        matches = matches and installed.get("bundle_id") == payload["bundle_id"]
        expected = {entry["path"]: entry["sha256"] for entry in payload["files"]}
        matches = matches and all(
            installed.get("hashes", {}).get(name) == expected[path]
            for name, path in PLATFORM_MEMBERS.items()
        )
    else:
        manifest = Path(release["directory"]) / names(kind, release["version"])[1]
        matches = (
            matches
            and installed.get("hashes", {}).get("manifest")
            == hashlib.sha256(manifest.read_bytes()).hexdigest()
        )
    return not matches


def device_lock(identity):
    key = hashlib.sha256(identity.encode()).hexdigest()
    return lock(root() / "devices" / f"{key}.lock")


def apply_updates(agent, identity, pair, run, attended, *, already_locked=False):
    device_key = hashlib.sha256(identity.encode()).hexdigest()
    with nullcontext() if already_locked else device_lock(identity):
        pending_path = root() / "devices" / f"{device_key}-pending.json"
        previous = json.loads(pending_path.read_text()) if pending_path.exists() else {}
        payloads = {
            kind: verify(kind, pair[kind]) for kind in ("platform", "databases")
        }
        current = state(agent)
        # A confirmed reboot may have completed while the previous host lost its reply.
        for stage in current["stages"]:
            pending = stage["pending"]
            kind = (
                "databases"
                if pending and pending.get("kind") == "databases"
                else "platform"
            )
            journal_path = Path(previous.get("run", "")) / kind / "publication.json"
            journal = (
                json.loads(journal_path.read_text())
                if previous and journal_path.is_file()
                else {}
            )
            if (
                (attended or kind == "databases")
                and journal.get("stage") == stage["stage"]
                and pending
                and pending.get("layout") == "dev"
                and (
                    kind == "databases" or pending.get("boot_id") != current["boot_id"]
                )
            ):
                reply, _ = agent._request(
                    "publication-control",
                    {"stage": stage["stage"], "action": "finish", "attended": True},
                    attempts=1,
                    timeout=60,
                )
                if reply.operation != "publication-complete":
                    raise AgentError.from_fields(reply.fields)
            else:
                raise AgentError(
                    f"unfinished publication {stage['stage']}; inspect/restore the saved stage before retrying; no reboot repeated"
                )
        current = state(agent) if current["stages"] else current
        platform_needed = needed(
            current["platform"], pair["platform"], "platform", payloads["platform"]
        )
        databases_needed = needed(
            current["databases"], pair["databases"], "databases", payloads["databases"]
        )
        if not platform_needed and current["platform"].get("active") is not True:
            raise AgentError(
                "installed platform is not active and healthy; inspect device state before deploying; no reboot attempted"
            )
        if platform_needed and not attended:
            raise AgentError(
                "queued platform update requires attendance; run: scripts/magik deploy --attended"
            )
        if platform_needed:
            if (
                current["configured_main"] != "MiSTer_MagiKDev"
                or current["running"].get("executable_path")
                != "/media/fat/MiSTer_MagiKDev"
            ):
                raise AgentError(
                    "queued platform requires running and selected Dev mode; select the intended mode explicitly"
                )
        with tempfile.TemporaryDirectory(prefix="magik-update-") as temporary:
            directory = Path(temporary)
            db_files = (
                database_files(pair, directory / "databases")
                if databases_needed
                else None
            )
            platform_files = prepare(pair, directory) if platform_needed else None
            if databases_needed and not platform_needed:
                package = repository() / "apps/mister"
                ensure_arm_application(package, package / "target/magik-build.json")
            if platform_files or db_files:
                atomic_json(pending_path, {"desired": pair, "run": str(run.resolve())})
            for kind, files in (("platform", platform_files), ("databases", db_files)):
                if files is None:
                    continue
                evidence = run / kind
                evidence.mkdir(exist_ok=True)
                atomic_json(
                    evidence / "run.json",
                    {"operation": kind, "source": {"device_identity": identity}},
                )
                fields = {"kind": kind, "layout": "dev"}
                if kind == "platform":
                    fields.update(attended=True, activate_fpga=True)
                print(f"Installing {pair[kind]['tag']}")
                publish(agent, evidence, files, **fields)
                current = state(agent)
                if needed(current[kind], pair[kind], kind, payloads[kind]):
                    raise AgentError(
                        f"{kind} installation did not verify; evidence: {evidence}"
                    )
        atomic_json(
            root() / "devices" / f"{device_key}.json",
            {
                "identity": identity,
                "desired": pair,
                "verified": current,
                "run": str(run.resolve()),
            },
        )
        pending_path.unlink(missing_ok=True)
