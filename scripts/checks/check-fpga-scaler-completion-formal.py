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


def yosys_prefix(
    netlist: Path,
    wrapper: Path,
    *,
    top: str = "mister_magik_ascal_completion_formal",
    define: str | None = None,
) -> str:
    define_option = f" -D{define}" if define else ""
    return "; ".join(
        (
            f"read_verilog -formal {netlist}",
            f"read_verilog -formal -sv{define_option} {wrapper}",
            f"hierarchy -check -top {top}",
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
    parser.add_argument("--solver-timeout", type=int, default=180)
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
    tail_formal_dut = source_dir / "mister_magik_scaler_copy_tail_formal_dut.vhd"
    tail_formal_wrapper = source_dir / "mister_magik_scaler_copy_tail_formal.sv"
    liveness_rtl = source_dir / "mister_magik_video_diagnostics_control.sv"
    liveness_formal_wrapper = (
        source_dir / "mister_magik_scaler_fetch_liveness_formal.sv"
    )
    pin = (source_dir / "Menu_MiSTer.commit").read_text().strip()

    root_commit = run(["git", "rev-parse", "HEAD"], cwd=root, capture=True).strip()
    menu = args.menu_dir.resolve()
    menu_commit = run(["git", "rev-parse", "HEAD"], cwd=menu, capture=True).strip()
    if not args.allow_unpinned and menu_commit != pin:
        fail(f"Menu commit {menu_commit} is not pinned {pin}")

    required = (
        patch,
        queue_tb,
        formal_dut,
        formal_wrapper,
        tail_formal_dut,
        tail_formal_wrapper,
        liveness_rtl,
        liveness_formal_wrapper,
        menu / "sys/ascal.vhd",
    )
    missing = [str(path) for path in required if not path.is_file()]
    if missing:
        fail("missing proof input: " + ", ".join(missing))

    formal_dut_source = formal_dut.read_text()
    for fragment in (
        "avl_reset_n<='0' WHEN reset_n='0' ELSE",
        "o_reset_n<='0' WHEN reset_n='0' ELSE",
        "request_meta<=request_toggle;",
        "request_sync<=request_meta;",
        "completion_pulse<=request_meta XOR request_sync;",
        "ack_meta<=request_sync;",
        "ack_sync<=ack_meta;",
    ):
        if fragment not in formal_dut_source:
            fail(f"formal DUT pipeline binding is missing: {fragment}")
    formal_wrapper_source = formal_wrapper.read_text()
    for fragment in (
        "assert(return_phase < BLEN);",
        "assert(write_phase < MAX_WORDS);",
        "if (request_toggle == request_sync)",
        "assert(request_meta == request_sync);",
        "if (completion_pulse)",
        "if (request_sync == ack_sync)",
        "assert(ack_meta == ack_sync);",
        "if (request_toggle == ack_sync)",
        "assert(request_sync == request_toggle);",
    ):
        if fragment not in formal_wrapper_source:
            fail(f"formal pipeline invariant binding is missing: {fragment}")
    liveness_formal_source = liveness_formal_wrapper.read_text()
    for fragment in (
        "assert(fifo_count <= 2);",
        "assert(return_phase < 128);",
        "assert(!no_request_seen || !snapshot_pending);",
        "assert(!record_ready || terminal_record_started);",
        "assert(!publish_crc_busy || publish_crc_phase <= 5'd30);",
        "assert(!terminal_record_started || first_stall_valid || observer_fault);",
        "case({$past(enqueue), $past(dequeue)})",
        "assert(frozen_state == $past(frozen_state));",
        "assert(publication_sequence == 4'd0);",
        "assert(published_bundle[31:0] == $past(published_bundle[31:0]));",
        "if($past(record_ready)) begin",
        "if(record_ready != $past(record_ready)) begin",
        "assert($past(publish_crc_phase) == 5'd30);",
        "watchdog_terminal && expected_progress",
        "cover(drained_during_reset);",
        "cover(enqueue && dequeue);",
    ):
        if fragment not in liveness_formal_source:
            fail(f"liveness observer formal obligation is missing: {fragment}")

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
        patched_source = patched_ascal.read_text()
        for fragment in (
            "FUNCTION read_obligation_accept(",
            "issued_v:=read_obligation_accept(",
            "avl_read_i,avl_read_accepted,avl_waitrequest,",
            "avl_reset_na='0' OR avl_state=sREAD);",
            "IF avl_read_i='0' THEN",
            "ELSIF issued_v THEN",
            "avl_read<=avl_read_i AND NOT avl_read_accepted",
            "WHEN avl_reset_na='0' OR avl_state=sREAD ELSE '0';",
            "IF avl_readdatavalid='1' AND avl_return_drain='0' THEN",
        ):
            if fragment not in patched_source:
                fail(f"patched production scheduler binding is missing: {fragment}")
        avalon_reset = re.search(
            r"IF avl_reset_na='0' THEN(?P<body>.*?)ELSIF rising_edge\(avl_clk\) THEN",
            patched_source,
            re.DOTALL,
        )
        if avalon_reset is None:
            fail("patched production Avalon reset branch is missing")
        for retained in (
            "avl_return_credits",
            "avl_return_phase",
            "avl_read_accepted",
        ):
            if retained in avalon_reset.group("body"):
                fail(f"production reset does not retain {retained}")
        if "avl_wad<=2*BLEN-1;" in avalon_reset.group("body"):
            fail(
                "production reset retains the write phase instead of pipelining release"
            )
        if avalon_reset.group("body").count("avl_return_release_pending<='0';") != 1:
            fail("production reset does not clear the release pipeline")
        guarded_drain_release = re.search(
            r"IF \(avl_o_vs_sync='0' AND avl_o_vs='1' AND\s*"
            r"return_drain_ready\(avl_return_credits,avl_return_phase\)\) OR\s*"
            r"\(avl_return_drain='1' AND avl_return_release_pending='1'\) THEN\s*"
            r"avl_wad<=2\*BLEN-1;\s*"
            r"END IF;\s*"
            r"IF avl_return_drain='1' THEN\s*"
            r"IF avl_return_release_pending='1' THEN\s*"
            r"avl_return_drain<='0';\s*"
            r"avl_return_release_pending<='0';\s*"
            r"ELSIF return_drain_ready\(\s*"
            r"avl_return_credits,avl_return_phase\) THEN\s*"
            r"avl_return_release_pending<='1';\s*END IF;\s*END IF;",
            patched_source,
        )
        if guarded_drain_release is None:
            fail("production drain release does not use one pipelined alignment cone")

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
                str(tail_formal_dut),
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
        tail_netlist_text = run(
            [
                "ghdl",
                "synth",
                "--std=08",
                f"--workdir={ghdl_work}",
                "--out=verilog",
                "mister_magik_scaler_copy_tail_formal_dut",
            ],
            cwd=ghdl_work,
            capture=True,
        )
        tail_netlist = temporary / "mister-magik-scaler-copy-tail-formal.v"
        tail_netlist.write_text(tail_netlist_text)
        if not re.search(
            r"module\s+mister_magik_scaler_copy_tail_formal_dut\b",
            tail_netlist_text,
        ):
            fail("GHDL synthesis did not emit the copy-tail formal DUT")
        if artifacts is not None:
            shutil.copy2(patched_ascal, artifacts / "patched-ascal.vhd")
            shutil.copy2(netlist, artifacts / "formal-dut.v")
            shutil.copy2(tail_netlist, artifacts / "copy-tail-formal-dut.v")
            shutil.copy2(liveness_rtl, artifacts / "scaler-fetch-liveness.sv")
            shutil.copy2(
                liveness_formal_wrapper,
                artifacts / "scaler-fetch-liveness-formal.sv",
            )

        liveness_prefix = "; ".join(
            (
                f"read_verilog -formal -sv -DFORMAL -I{source_dir} {liveness_rtl}",
                f"read_verilog -formal -sv {liveness_formal_wrapper}",
                "hierarchy -check -top mister_magik_scaler_fetch_liveness_formal",
                "proc",
                "flatten",
                "clk2fflogic",
                "opt_clean",
            )
        )
        liveness_base_command = (
            liveness_prefix
            + "; chformal -cover -remove"
            + "; sat -seq 32 -set-assumes -set-init-zero"
            + " -prove-asserts -verify"
            + f" -timeout {args.solver_timeout}"
        )
        liveness_base_log = run_solver(
            [yosys, "-Q", "-p", liveness_base_command],
            cwd=root,
            log_path=artifacts / "scaler-fetch-liveness-base.log"
            if artifacts
            else None,
        )
        if "SAT proof finished - no model found" not in liveness_base_log:
            fail("Yosys did not complete the liveness observer bounded proof")

        liveness_covers = {
            "drained_during_reset": 20,
            "first_stall_valid": 24,
            "observer_fault": 8,
            "snapshot_completed_seen": 20,
            # formal_clk consumes two solver steps per observed Avalon edge;
            # allow one accepted burst, all 128 returns, and the replacement.
            "simultaneous_event_seen": 270,
        }
        liveness_cover_results: dict[str, int] = {}
        for witness, depth in liveness_covers.items():
            liveness_cover_command = (
                liveness_prefix
                + "; chformal -cover -remove; chformal -assert -remove"
                + f"; sat -seq {depth} -set-assumes -set-init-zero"
                + f" -set-at {depth} {witness} 1"
                + f" -timeout {args.solver_timeout} -show {witness}"
            )
            liveness_cover_log = run_solver(
                [yosys, "-Q", "-p", liveness_cover_command],
                cwd=root,
                log_path=(
                    artifacts / f"scaler-fetch-liveness-{witness}.log"
                    if artifacts
                    else None
                ),
            )
            if "SAT solving finished - model found" not in liveness_cover_log:
                fail(f"required liveness observer cover is unreachable: {witness}")
            liveness_cover_results[witness] = depth

        if not args.preflight:
            liveness_induction_command = (
                liveness_prefix
                + "; chformal -cover -remove"
                + "; sat -seq 1 -tempinduct -set-assumes -set-init-zero"
                + " -prove-asserts -verify"
                + f" -maxsteps {args.safety_maxsteps}"
                + f" -timeout {args.solver_timeout}"
            )
            liveness_induction_log = run_solver(
                [yosys, "-Q", "-p", liveness_induction_command],
                cwd=root,
                log_path=(
                    artifacts / "scaler-fetch-liveness-induction.log"
                    if artifacts
                    else None
                ),
            )
            if not any(
                marker in liveness_induction_log
                for marker in (
                    "Temporal induction proof finished - no model found",
                    "Induction step proven: SUCCESS!",
                )
            ):
                fail("Yosys did not complete liveness observer induction")

        tail_prefix = yosys_prefix(
            tail_netlist,
            tail_formal_wrapper,
            top="mister_magik_scaler_copy_tail_formal",
        )
        tail_base_command = (
            tail_prefix
            + "; chformal -cover -remove"
            + "; sat -seq 48 -set-assumes -set-init-zero"
            + " -prove-asserts -verify"
            + f" -timeout {args.solver_timeout}"
        )
        tail_base_log = run_solver(
            [yosys, "-Q", "-p", tail_base_command],
            cwd=root,
            log_path=artifacts / "copy-tail-base.log" if artifacts else None,
        )
        if "SAT proof finished - no model found" not in tail_base_log:
            fail("Yosys did not complete the copy-tail bounded proof")

        tail_cover_command = (
            tail_prefix
            + "; chformal -cover -remove; chformal -assert -remove"
            + "; sat -seq 48 -set-assumes -set-init-zero"
            + " -set-at 48 retired 1"
            + f" -timeout {args.solver_timeout} -show retired"
        )
        tail_cover_log = run_solver(
            [yosys, "-Q", "-p", tail_cover_command],
            cwd=root,
            log_path=artifacts / "copy-tail-cover.log" if artifacts else None,
        )
        if "SAT solving finished - model found" not in tail_cover_log:
            fail("required copy-tail retirement cover is unreachable")

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
            if artifacts is not None:
                safety_command += (
                    f" -dump_vcd {artifacts / 'induction.vcd'} -show-public"
                )
            safety_log = run_solver(
                [yosys, "-Q", "-p", safety_command],
                cwd=root,
                log_path=artifacts / "induction.log" if artifacts else None,
            )
            if not any(
                marker in safety_log
                for marker in (
                    "Temporal induction proof finished - no model found",
                    "Induction step proven: SUCCESS!",
                )
            ):
                fail("Yosys did not report a completed temporal induction proof")

        cover_witnesses = {
            "cover_two_stopped_delivered": ("COVER_WITNESS_TWO_STOPPED", 620),
            "cover_coincident_ack_completion": ("COVER_WITNESS_COINCIDENT", 530),
            "cover_final_old_beat_during_reset": ("COVER_WITNESS_FINAL_RESET", 270),
            "cover_old_beat_after_reset": ("COVER_WITNESS_OLD_POST_RESET", 20),
            "cover_vs_alignment_during_drain": ("COVER_WITNESS_VS_ALIGN", 10),
            "cover_drain_release_without_vs": ("COVER_WITNESS_DRAIN_NO_VS", 10),
            "cover_first_post_drain_completion": (
                "COVER_WITNESS_FIRST_COMPLETION",
                270,
            ),
            "cover_active_credit_vs": ("COVER_WITNESS_ACTIVE_CREDIT_VS", 20),
            "cover_issue_empty_vs": ("COVER_WITNESS_ISSUE_EMPTY_VS", 20),
            "cover_final_return_vs_wait": (
                "COVER_WITNESS_FINAL_RETURN_VS_WAIT",
                # One full return burst plus reset/release/scheduler overhead.
                320,
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
            "copy_tail_formal_dut_sha256": sha256(tail_formal_dut),
            "copy_tail_formal_wrapper_sha256": sha256(tail_formal_wrapper),
            "scaler_fetch_liveness_rtl_sha256": sha256(liveness_rtl),
            "scaler_fetch_liveness_formal_wrapper_sha256": sha256(
                liveness_formal_wrapper
            ),
            "ghdl_netlist_sha256": sha256(netlist),
            "copy_tail_ghdl_netlist_sha256": sha256(tail_netlist),
            "blen": 128,
            "reset_reachable_base_depth": args.base_depth,
            "safety_induction_maxsteps": args.safety_maxsteps,
            "copy_tail_bounded_depth": 48,
            "copy_tail_retirement_cover_depth": 48,
            "scaler_fetch_liveness_bounded_depth": 32,
            "scaler_fetch_liveness_induction_maxsteps": args.safety_maxsteps,
            "scaler_fetch_liveness_covers": liveness_cover_results,
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
