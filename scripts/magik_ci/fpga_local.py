"""Frozen-source, fixed-seed Apple-container FPGA signoff.

Completed evidence is immutable. A failed stage stays in its unique run folder;
this runner never deletes or reuses an incomplete variant.
"""

from __future__ import annotations
import hashlib
import json
import os
import re
import shlex
import subprocess
import sys
from datetime import datetime, timezone
from pathlib import Path

PROFILES = {
    "production-v1": [],
    "experimental_raw_scaler-v1": ["--experimental-diagnostic"],
    "experimental_scaler_fetch-v1": ["--experimental-scaler-fetch"],
    "experimental_scaler_causal-v1": ["--experimental-scaler-causal"],
}


def run(command, *, cwd, env=None, log=None):
    if log:
        with log.open("w") as stream:
            subprocess.run(
                command,
                cwd=cwd,
                env=env,
                stdout=stream,
                stderr=subprocess.STDOUT,
                check=True,
            )
        return ""
    return subprocess.check_output(command, cwd=cwd, env=env, text=True).strip()


def sha(path):
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for block in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(block)
    return digest.hexdigest()


def local_root(root, override=None):
    value = override or os.environ.get("MISTER_FPGA_LOCAL_ROOT")
    if value:
        path = Path(value)
        if not path.is_absolute():
            raise ValueError("local root must be absolute")
        return path.resolve()
    common = Path(
        run(
            ["git", "rev-parse", "--path-format=absolute", "--git-common-dir"], cwd=root
        )
    )
    return common.parent / "build/fpga-local-apple"


def identity(root):
    commit = run(["git", "rev-parse", "HEAD"], cwd=root)
    if commit != run(["git", "rev-parse", "refs/heads/main^0"], cwd=root):
        raise ValueError("freeze the candidate commit on local main before signoff")
    if run(["git", "status", "--porcelain", "--untracked-files=normal"], cwd=root):
        raise ValueError("signoff requires a clean committed checkout")
    rtl = root / "mister/platform/fpga/menu-vblank-latch"
    pins = {
        key: (rtl / name).read_text().strip()
        for key, name in (
            ("menu", "Menu_MiSTer.commit"),
            ("baseline", "video-diagnostics-baseline.commit"),
            ("seed", "Quartus.seed"),
            ("profile", "local-signoff-profile.txt"),
        )
    }
    if not re.fullmatch(r"[1-9][0-9]*", pins["seed"]):
        raise ValueError("invalid canonical seed")
    if pins["profile"] not in PROFILES:
        raise ValueError("unsupported signoff profile")
    date = (root / "scripts/quartus/fpga-build-epoch-v1.txt").read_text().strip()
    if not re.fullmatch(r"[0-9]{6}", date):
        raise ValueError("invalid pinned build date")
    return dict(
        pins,
        commit=commit,
        date=date,
        prepare_sha256=sha(root / "scripts/prepare-fpga-menu-signoff.py"),
    )


def clone_at(source, destination, revision, cwd):
    run(
        ["git", "clone", "--shared", "--no-checkout", str(source), str(destination)],
        cwd=cwd,
    )
    run(["git", "checkout", "--detach", revision], cwd=destination)


def signoff(root: Path, cache: Path, menu_source: Path | None = None) -> int:
    frozen = identity(root)
    install = cache / "quartus-lite-17.0/apple-intelFPGA_lite"
    image = os.environ.get(
        "QUARTUS_APPLE_IMAGE", "mister-magik-quartus17-apple:ubuntu18-amd64"
    )
    if not (install / "17.0/quartus/bin/quartus_sh").is_file():
        raise ValueError(
            "Quartus runtime missing; run scripts/magik-platform fpga setup"
        )
    image_info = json.loads(run(["container", "image", "inspect", image], cwd=root))
    version = run(
        [
            "container",
            "run",
            "--arch",
            "amd64",
            "--rm",
            "--mount",
            f"type=bind,source={install},target=/opt/intelFPGA_lite,readonly",
            image,
            "quartus_sh",
            "--version",
        ],
        cwd=root,
    )
    if not re.search(r"17\.0\.0.*Build 595", version):
        raise ValueError("runtime is not pinned Quartus 17.0 Build 595")
    frozen.update(quartus_version=version, container_image=image_info)
    cache.mkdir(parents=True, exist_ok=True)
    stamp = datetime.now(timezone.utc).strftime("%Y%m%dT%H%M%S%fZ")
    output = cache / "runs" / f"{frozen['commit'][:12]}-{stamp}"
    output.mkdir(parents=True)
    (output / "inputs.json").write_text(json.dumps(frozen, indent=2) + "\n")
    sources = output / "sources"
    sources.mkdir()
    candidate = sources / "candidate"
    baseline = sources / "baseline"
    menu = sources / "menu"
    clone_at(root, candidate, frozen["commit"], root)
    clone_at(root, baseline, frozen["baseline"], root)
    if menu_source:
        clone_at(menu_source, menu, frozen["menu"], root)
    else:
        run(
            [
                "git",
                "clone",
                "--no-checkout",
                "https://github.com/MiSTer-devel/Menu_MiSTer.git",
                str(menu),
            ],
            cwd=root,
        )
        run(["git", "checkout", "--detach", frozen["menu"]], cwd=menu)
    proofs = output / "proofs"
    proofs.mkdir()
    gates = [
        (
            "integration",
            [
                sys.executable,
                str(candidate / "scripts/checks/check-fpga-latch-integration.py"),
                str(menu),
                "--simulate",
            ],
        ),
        (
            "scheduler",
            [
                sys.executable,
                str(
                    candidate / "scripts/checks/check-fpga-scaler-completion-formal.py"
                ),
                str(menu),
                "--artifacts-dir",
                str(proofs / "scheduler"),
                "--report",
                str(proofs / "scheduler.json"),
            ],
        ),
        (
            "observer",
            [
                sys.executable,
                str(candidate / "scripts/checks/check-fpga-causal-evidence.py"),
                "--artifacts-dir",
                str(proofs / "observer"),
            ],
        ),
        (
            "boundary-replay",
            [
                sys.executable,
                str(candidate / "scripts/checks/check-fpga-terminator-boundary.py"),
                str(menu),
                "--scheduler-artifacts",
                str(proofs / "scheduler"),
                "--artifacts-dir",
                str(proofs / "boundary-replay"),
                "--replay-only",
            ],
        ),
    ]
    for name, command in gates:
        print("Validating frozen candidate: " + name, flush=True)
        run(command, cwd=candidate, log=proofs / (name + ".log"))
    run(
        [
            sys.executable,
            str(candidate / "scripts/prepare-fpga-menu-signoff.py"),
            str(menu),
        ],
        cwd=root,
    )
    wrappers = output / "bin"
    wrappers.mkdir()
    for tool in ("quartus_sh", "quartus_sta"):
        path = wrappers / tool
        path.write_text(
            "#!/bin/bash\nset -euo pipefail\nexec container run --arch amd64 --rm --cpus 4 --memory 12g "
            + shlex.join(
                [
                    "--mount",
                    f"type=bind,source={install},target=/opt/intelFPGA_lite,readonly",
                ]
            )
            + ' --mount "type=bind,source=$PWD,target=/work" --workdir /work '
            + shlex.join([image, tool])
            + ' "$@"\n'
        )
        path.chmod(0o755)
    env = dict(
        os.environ,
        PATH=str(wrappers) + os.pathsep + os.environ["PATH"],
        MISTER_FPGA_LOCAL_SIGNOFF="1",
        MISTER_FPGA_APPLE_WRAPPER_DIR=str(wrappers),
        MISTER_MENU_DIR=str(menu),
        MISTER_FPGA_QUALIFIED_MAGIK_REVISION=frozen["commit"],
        MISTER_FPGA_BUILD_DATE=frozen["date"],
        MISTER_FPGA_QUARTUS_SEED=frozen["seed"],
        QUARTUS_STRACE="0",
    )
    env.pop("GITHUB_ACTIONS", None)
    reports = {}
    for variant, source, patched in [
        ("stock", candidate, "0"),
        ("baseline", baseline, "1"),
        ("patched", candidate, "1"),
    ]:
        stage = output / ("." + variant + ".building")
        stage.mkdir()
        driver = source / "scripts/build-fpga-vblank-latch-core.sh"
        if variant == "baseline":
            # Only adapt the old driver's invocation guard. Its RTL, constraints,
            # seed, date and Quartus commands are unchanged and hashed below.
            content = driver.read_text()
            guard = 'if [[ "${GITHUB_ACTIONS:-}" != "true" ]]; then'
            if content.count(guard) != 1:
                raise ValueError("pinned baseline build guard changed")
            adapter = source / "scripts/build-fpga-apple-signoff-adapter.sh"
            adapter.write_text(
                content.replace(
                    guard,
                    'if [[ "${GITHUB_ACTIONS:-}" != "true" && "${MISTER_FPGA_LOCAL_SIGNOFF:-}" != "1" ]]; then',
                )
            )
            driver = adapter
        variant_env = dict(
            env,
            MISTER_FPGA_APPLY_PATCH=patched,
            MISTER_FPGA_OUT_DIR=str(stage),
            MISTER_MENU_BUILD_DIR=str(stage / "Menu-work"),
        )
        print(f"Building {variant}: {stage}", flush=True)
        run(
            ["bash", str(driver)], cwd=source, env=variant_env, log=stage / "driver.log"
        )
        if not (stage / "menu-magik-vblank-latch.rbf").is_file():
            raise ValueError("build returned without an RBF")
        evidence = {
            str(p.relative_to(stage)): sha(p)
            for p in sorted(stage.rglob("*"))
            if p.is_file()
            and not any(
                part in {".git", "db", "incremental_db"}
                for part in p.relative_to(stage).parts
            )
        }
        (stage / "completed.json").write_text(
            json.dumps(
                dict(
                    inputs=frozen,
                    variant=variant,
                    driver_sha256=sha(driver),
                    files=evidence,
                ),
                indent=2,
            )
            + "\n"
        )
        completed = output / variant
        stage.rename(completed)
        reports[variant] = sorted(
            p
            for folder in (completed, completed / "reports")
            for p in folder.glob("*")
            if p.is_file() and p.suffix in {".log", ".rpt", ".summary"}
        )
    command = [
        sys.executable,
        str(candidate / "scripts/checks/check-fpga-quartus-delta.py"),
        *PROFILES[frozen["profile"]],
        "--json",
    ]
    for variant, paths in reports.items():
        for path in paths:
            command += ["--" + variant, str(path)]
    run(command, cwd=candidate, log=output / "quartus-delta.json")
    certificate = {
        "result": "pass",
        "inputs": frozen,
        "delta_sha256": sha(output / "quartus-delta.json"),
        "variants": {v: sha(output / v / "completed.json") for v in reports},
        "proof_files": {
            str(p.relative_to(proofs)): sha(p) for p in proofs.rglob("*") if p.is_file()
        },
    }
    (output / "signoff.json").write_text(json.dumps(certificate, indent=2) + "\n")
    print(output / "signoff.json")
    return 0
