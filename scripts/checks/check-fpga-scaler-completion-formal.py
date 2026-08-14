#!/usr/bin/env python3
# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

"""Prove the exact-source scaler completion transport and reset accounting."""

from __future__ import annotations

import argparse
import hashlib
import json
import re
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path


def fail(message: str) -> None:
    print(f"FPGA scaler completion formal check failed: {message}", file=sys.stderr)
    raise SystemExit(1)


def sha256(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def run(command: list[str], *, cwd: Path, capture: bool = False) -> str:
    try:
        result = subprocess.run(
            command,
            cwd=cwd,
            check=True,
            text=True,
            stdout=subprocess.PIPE if capture else None,
            stderr=subprocess.STDOUT if capture else None,
        )
    except subprocess.CalledProcessError as error:
        if error.stdout:
            print(error.stdout, file=sys.stderr, end="")
        fail(f"command exited {error.returncode}: {' '.join(command)}")
    return result.stdout or ""


def run_solver(command: list[str], *, cwd: Path, log_path: Path | None) -> str:
    result = subprocess.run(
        command,
        cwd=cwd,
        check=False,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
    )
    output = result.stdout or ""
    if log_path is not None:
        log_path.write_text(output)
    if result.returncode != 0:
        tail = "\n".join(output.splitlines()[-120:])
        if tail:
            print(tail, file=sys.stderr)
        fail(f"solver exited {result.returncode}; see {log_path or 'captured output'}")
    return output


def yosys_binary() -> str:
    installed = shutil.which("yosys")
    if installed:
        return installed
    homebrew = Path("/opt/homebrew/bin/yosys")
    if homebrew.is_file():
        return str(homebrew)
    fail("Yosys is unavailable")


def yosys_prefix(netlist: Path, wrapper: Path, *, define: str | None = None) -> str:
    define_option = f" -D{define}" if define else ""
    return "; ".join(
        (
            f"read_verilog -formal {netlist}",
            f"read_verilog -formal -sv{define_option} {wrapper}",
            "hierarchy -check -top mister_magik_ascal_completion_formal",
            "proc",
            "flatten",
            "clk2fflogic",
            "opt_clean",
        )
    )


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("menu_dir", type=Path)
    parser.add_argument("--allow-unpinned", action="store_true")
    parser.add_argument("--preflight", action="store_true")
    parser.add_argument("--base-depth", type=int, default=24)
    parser.add_argument("--safety-maxsteps", type=int, default=32)
    parser.add_argument("--cover-depth", type=int, default=700)
    parser.add_argument("--solver-timeout", type=int, default=90)
    parser.add_argument("--artifacts-dir", type=Path)
    parser.add_argument("--report", type=Path)
    args = parser.parse_args()

    artifacts = args.artifacts_dir.resolve() if args.artifacts_dir else None
    if artifacts is not None:
        artifacts.mkdir(parents=True, exist_ok=True)

    if (
        args.base_depth < 1
        or args.safety_maxsteps < 1
        or args.cover_depth < 1
        or args.solver_timeout < 1
    ):
        fail("proof depths and timeout must be positive")
    if shutil.which("ghdl") is None:
        fail("GHDL is unavailable")
    yosys = yosys_binary()

    root = Path(__file__).resolve().parents[2]
    source_dir = root / "mister/platform/fpga/menu-vblank-latch"
    patch = source_dir / "Menu_MiSTer-vblank-latched-fbuf.patch"
    queue_tb = source_dir / "tb_mister_magik_scaler_completion_queue.vhd"
    formal_dut = source_dir / "mister_magik_scaler_completion_formal_dut.vhd"
    formal_wrapper = source_dir / "mister_magik_ascal_completion_formal.sv"
    pin = (source_dir / "Menu_MiSTer.commit").read_text().strip()

    root_commit = run(["git", "rev-parse", "HEAD"], cwd=root, capture=True).strip()
    menu = args.menu_dir.resolve()
    menu_commit = run(
        ["git", "rev-parse", "HEAD"], cwd=menu, capture=True
    ).strip()
    if not args.allow_unpinned and menu_commit != pin:
        fail(f"Menu commit {menu_commit} is not pinned {pin}")

    required = (patch, queue_tb, formal_dut, formal_wrapper, menu / "sys/ascal.vhd")
    missing = [str(path) for path in required if not path.is_file()]
    if missing:
        fail("missing proof input: " + ", ".join(missing))

    with tempfile.TemporaryDirectory(prefix="mister-magik-scaler-formal-") as temp:
        temporary = Path(temp)
        work = temporary / "Menu_MiSTer"
        shutil.copytree(
            menu,
            work,
            ignore=shutil.ignore_patterns(".git", "db", "output_files"),
        )
        run(["git", "apply", "--recount", "--check", str(patch)], cwd=work)
        run(["git", "apply", "--recount", str(patch)], cwd=work)
        patched_ascal = work / "sys/ascal.vhd"

        ghdl_work = temporary / "ghdl-work"
        ghdl_work.mkdir()
        run(
            [
                "ghdl",
                "-a",
                "--std=08",
                f"--workdir={ghdl_work}",
                str(patched_ascal),
                str(queue_tb),
                str(formal_dut),
            ],
            cwd=ghdl_work,
        )
        run(
            [
                "ghdl",
                "-e",
                "--std=08",
                f"--workdir={ghdl_work}",
                "tb_mister_magik_scaler_completion_queue",
            ],
            cwd=ghdl_work,
        )
        run(
            [
                "ghdl",
                "-r",
                "--std=08",
                f"--workdir={ghdl_work}",
                "tb_mister_magik_scaler_completion_queue",
                "--assert-level=error",
            ],
            cwd=ghdl_work,
        )
        netlist_text = run(
            [
                "ghdl",
                "synth",
                "--std=08",
                f"--workdir={ghdl_work}",
                "--out=verilog",
                "-gBLEN=128",
                "mister_magik_scaler_completion_formal_dut",
            ],
            cwd=ghdl_work,
            capture=True,
        )
        netlist = temporary / "mister-magik-scaler-completion-formal.v"
        netlist.write_text(netlist_text)
        if not re.search(
            r"module\s+mister_magik_scaler_completion_formal_dut\b", netlist_text
        ):
            fail("GHDL synthesis did not emit the narrow formal DUT")
        if artifacts is not None:
            shutil.copy2(patched_ascal, artifacts / "patched-ascal.vhd")
            shutil.copy2(netlist, artifacts / "formal-dut.v")

        prefix = yosys_prefix(netlist, formal_wrapper)
        base_command = (
            prefix
            + "; chformal -cover -remove"
            + f"; sat -seq {args.base_depth} -set-assumes -set-init-zero"
            + " -prove-asserts -verify"
            + f" -timeout {args.solver_timeout}"
        )
        if artifacts is not None:
            base_command += f" -dump_vcd {artifacts / 'base.vcd'} -show-public"
        base_log = run_solver(
            [yosys, "-Q", "-p", base_command],
            cwd=root,
            log_path=artifacts / "base.log" if artifacts else None,
        )
        if "SAT proof finished - no model found" not in base_log:
            fail("Yosys did not report a completed reset-reachable base proof")

        if not args.preflight:
            safety_command = (
                prefix
                + "; chformal -cover -remove"
                + "; sat -seq 1 -tempinduct -set-assumes -set-init-zero"
                + " -prove-asserts -verify"
                + f" -maxsteps {args.safety_maxsteps}"
                + f" -timeout {args.solver_timeout}"
            )
            safety_log = run_solver(
                [yosys, "-Q", "-p", safety_command],
                cwd=root,
                log_path=artifacts / "induction.log" if artifacts else None,
            )
            if "Temporal induction proof finished - no model found" not in safety_log:
                fail("Yosys did not report a completed temporal induction proof")

        cover_witnesses = {
            "cover_two_stopped_delivered": ("COVER_WITNESS_TWO_STOPPED", 560),
            "cover_coincident_ack_completion": ("COVER_WITNESS_COINCIDENT", 530),
            "cover_final_old_beat_during_reset": ("COVER_WITNESS_FINAL_RESET", 270),
            "cover_old_beat_after_reset": ("COVER_WITNESS_OLD_POST_RESET", 20),
            "cover_vs_alignment_during_drain": ("COVER_WITNESS_VS_ALIGN", 10),
            "cover_first_post_drain_completion": (
                "COVER_WITNESS_FIRST_COMPLETION",
                270,
            ),
        }
        cover_results: dict[str, int] = {}
        for name, (define, witness_depth) in cover_witnesses.items():
            depth = min(args.cover_depth, witness_depth)
            cover_prefix = yosys_prefix(netlist, formal_wrapper, define=define)
            cover_command = (
                cover_prefix
                + "; chformal -cover -remove; chformal -assert -remove"
                + f"; sat -seq {depth} -set-assumes -set-init-zero"
                + f" -set-at {depth} {name} 1"
                + f" -timeout {args.solver_timeout} -show {name}"
            )
            cover_log = run_solver(
                [yosys, "-Q", "-p", cover_command],
                cwd=root,
                log_path=artifacts / f"{name}.log" if artifacts else None,
            )
            if "SAT solving finished - model found" not in cover_log:
                fail(f"required non-vacuity cover is unreachable: {name}")
            cover_results[name] = depth

        report = {
            "schema": "mister-magik-scaler-completion-formal-v1",
            "root_commit": root_commit,
            "menu_commit": menu_commit,
            "patch_sha256": sha256(patch),
            "patched_ascal_sha256": sha256(patched_ascal),
            "formal_dut_sha256": sha256(formal_dut),
            "formal_wrapper_sha256": sha256(formal_wrapper),
            "ghdl_netlist_sha256": sha256(netlist),
            "blen": 128,
            "reset_reachable_base_depth": args.base_depth,
            "safety_induction_maxsteps": args.safety_maxsteps,
            "covers": cover_results,
            "result": "preflight-pass" if args.preflight else "pass",
        }
        encoded = json.dumps(report, indent=2, sort_keys=True) + "\n"
        if args.report:
            args.report.parent.mkdir(parents=True, exist_ok=True)
            args.report.write_text(encoded)
        print(encoded, end="")


if __name__ == "__main__":
    main()
