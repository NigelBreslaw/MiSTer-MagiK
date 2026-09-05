"""Bounded, reproducible delivery measurements; results never gate deployment."""
from __future__ import annotations

import json
import math
import os
import subprocess
import time
from pathlib import Path


def summarize(attempts, target_ms):
    durations = sorted(row["elapsed_ms"] for row in attempts)
    failures = sum(row["exit_code"] != 0 for row in attempts)
    p95 = durations[math.ceil(len(durations) * .95) - 1] if durations else None
    return {"attempts": len(attempts), "failures": failures, "p95_ms": p95,
            "target_ms": target_ms, "target_met": bool(durations) and not failures and p95 <= target_ms}


def run_delivery_matrix(run: Path) -> int:
    """Twenty attempts per case, including failures, then restore original sources/app."""
    repository = Path(__file__).resolve().parents[3]
    probe = repository / "magik2/probe"
    rust = probe / "src/main.rs"
    slint = probe / "ui/probe.slint"
    originals = {path: path.read_bytes() for path in (rust, slint)}
    artifact = probe / "target/armv7-unknown-linux-gnueabihf/release/mister-magik2-probe"
    index = {"cases": {}, "restoration": None}
    base_env = dict(os.environ)
    base_env.pop("MISTER_MAGIK2_REPAIR", None)
    base_env.pop("MISTER_MAGIK2_PREBUILT_ARTIFACT", None)
    def save():
        (run / "acceptance.json").write_text(json.dumps(index, indent=2) + "\n")
    def attempt(case, number, extra=None):
        folder = run / case / str(number)
        folder.mkdir(parents=True)
        env = {**base_env, "MISTER_MAGIK2_RESULTS": str(folder.resolve()), **(extra or {})}
        started = time.monotonic()
        with (folder / "command.log").open("w") as output:
            try:
                result = subprocess.run([str(repository / "scripts/magik2"), "deploy"],
                    cwd=repository, env=env, stdout=output, stderr=subprocess.STDOUT, timeout=120)
                code = result.returncode
            except subprocess.TimeoutExpired:
                code = 124
        bundles = list(folder.glob("*/run.json"))
        return {"number": number, "exit_code": code,
                "elapsed_ms": round((time.monotonic()-started)*1000),
                "bundle": str(bundles[0].parent.relative_to(run)) if len(bundles) == 1 else None}
    try:
        index["warmup"] = attempt("warmup", 0)
        save()
        if index["warmup"]["exit_code"]:
            return 1
        original_artifact = artifact.read_bytes()
        for case, target in (("no-op", 1000), ("prebuilt", 5000), ("rust-edit", 15000), ("slint-edit", 15000)):
            rows = []
            index["cases"][case] = {"attempts": rows}
            for number in range(20):
                extra = {}
                if case == "prebuilt":
                    payload = run / "changed-probe"
                    payload.write_bytes(original_artifact + f"\nacceptance {number}\n".encode())
                    extra["MISTER_MAGIK2_PREBUILT_ARTIFACT"] = str(payload.resolve())
                elif case == "rust-edit":
                    rust.write_bytes(originals[rust].replace(b"magik2-probe ready width=", f"magik2-probe sample-{number} ready width=".encode(), 1))
                elif case == "slint-edit":
                    slint.write_bytes(originals[slint].replace(b'MiSTer MagiK 2 Probe"', f'MiSTer MagiK 2 Probe {number}"'.encode(), 1))
                rows.append(attempt(case, number, extra))
                index["cases"][case]["summary"] = summarize(rows, target)
                save()
            for path, original in originals.items():
                path.write_bytes(original)
            print(f"{case}: {index['cases'][case]['summary']}", flush=True)
    finally:
        for path, original in originals.items():
            path.write_bytes(original)
        (run / "changed-probe").unlink(missing_ok=True)
        index["restoration"] = attempt("restoration", 0)
        save()
    return int(index["restoration"]["exit_code"] != 0 or any(case["summary"]["failures"] for case in index["cases"].values()))
