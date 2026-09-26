#!/usr/bin/env python3
"""Check the pinned reset terminator against the production-bound scheduler.

Requires the exact-source completion checker artifacts. A counterexample is
retained evidence, never converted into a signoff pass.
"""

from __future__ import annotations
import argparse
import hashlib
import json
import re
from pathlib import Path
import subprocess


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("menu_dir", type=Path)
    parser.add_argument("--scheduler-artifacts", type=Path, required=True)
    parser.add_argument("--artifacts-dir", type=Path, required=True)
    parser.add_argument(
        "--replay-only",
        action="store_true",
        help="simulate the known boundary failure and require the observer to capture it",
    )
    parser.add_argument("--depth", type=int, default=64)
    args = parser.parse_args()
    root = Path(__file__).resolve().parents[2]
    rtl = root / "mister/platform/fpga/menu-vblank-latch"
    menu = args.menu_dir.resolve()
    pin = (rtl / "Menu_MiSTer.commit").read_text().strip()
    actual = subprocess.check_output(
        ["git", "-C", str(menu), "rev-parse", "HEAD"], text=True
    ).strip()
    if actual != pin:
        parser.error("Menu revision differs from pinned source")
    sysmem = (menu / "sys/sysmem.sv").read_text()
    for fragment in (
        "vbuf_reset_0 <= reset_out;",
        "vbuf_reset_1 <= vbuf_reset_0;",
        ".rst_req_sync             (vbuf_reset_1)",
        ".read_master              (f2h_vbuf_read)",
        ".read_slave               (vbuf_read)",
    ):
        if fragment not in sysmem:
            parser.error("sysmem binding changed: " + fragment)
    out = args.artifacts_dir.resolve()
    out.mkdir(parents=True, exist_ok=True)
    netlist = args.scheduler_artifacts.resolve() / "formal-dut.v"
    wrapper = rtl / "mister_magik_terminator_formal.sv"
    terminator = menu / "sys/f2sdram_safe_terminator.sv"
    # Yosys does not resolve the vendor's forward localparam references in ports.
    # Move the identical constant expressions into the parameter list; no logic changes.
    source = terminator.read_text()
    for line in (
        "localparam BYTEENABLE_WIDTH = DATA_WIDTH/8;",
        "localparam ADDRESS_WITDH    = 32-$clog2(BYTEENABLE_WIDTH);",
    ):
        if source.count(line) != 1:
            parser.error("terminator parameter declaration changed")
        source = source.replace(line, "")
    source = source.replace(
        "parameter     BURSTCOUNT_WIDTH = 8",
        "parameter BURSTCOUNT_WIDTH = 8,\n"
        "localparam BYTEENABLE_WIDTH = DATA_WIDTH/8,\n"
        "localparam ADDRESS_WITDH = 32-$clog2(BYTEENABLE_WIDTH)",
    )
    source = re.sub(r"\boutput\s+", "output logic ", source)
    source = re.sub(r"\bwire(\s+next_state_write;)", r"logic\1", source)
    declarations = re.findall(
        r"^reg \[(?:BURSTCOUNT_WIDTH|ADDRESS_WITDH)[^\n]+(?:write_burstcounter|write_burstcount_latch|write_address_latch)[^\n]+$",
        source,
        re.M,
    )
    if len(declarations) != 3:
        parser.error("terminator write-state declarations changed")
    for declaration in declarations:
        source = source.replace(declaration, "")
    source = source.replace(
        "reg  state_write", "\n".join(declarations) + "\nreg  state_write"
    )
    normalized = out / "terminator.sv"
    normalized.write_text(source)
    replay = rtl / "tb_mister_magik_terminator_boundary.sv"
    observer = rtl / "mister_magik_scaler_causal_state.sv"
    if args.replay_only:
        compile_result = subprocess.run(
            [
                "iverilog",
                "-g2012",
                "-s",
                "tb_mister_magik_terminator_boundary",
                "-o",
                str(out / "replay"),
                str(netlist),
                str(normalized),
                str(observer),
                str(replay),
            ],
            check=False,
            capture_output=True,
            text=True,
        )
        (out / "compile.log").write_text(compile_result.stdout + compile_result.stderr)
        if compile_result.returncode:
            return compile_result.returncode
        result = subprocess.run(
            ["vvp", str(out / "replay")],
            cwd=out,
            check=False,
            capture_output=True,
            text=True,
        )
        (out / "replay.log").write_text(result.stdout + result.stderr)
        passed = result.returncode == 0 and "PASS:" in result.stdout
        (out / "result.json").write_text(
            json.dumps(
                {
                    "result": "detected-known-boundary-failure" if passed else "failed",
                    "boundary_safe": False,
                    "menu_commit": actual,
                    "inputs": {
                        str(p): hashlib.sha256(p.read_bytes()).hexdigest()
                        for p in (netlist, terminator, normalized, observer, replay)
                    },
                },
                indent=2,
            )
            + "\n"
        )
        print(result.stdout)
        return 0 if passed else 1
    inputs = [netlist, wrapper, terminator, normalized, menu / "sys/sysmem.sv"]
    commands = [
        f"read_verilog -formal {netlist}",
        f"read_verilog -formal -sv -DPROVE_BOUNDARY {normalized} {wrapper}",
        "hierarchy -check -top mister_magik_terminator_formal",
        "proc",
        "flatten",
        "clk2fflogic",
        "opt_clean",
        "chformal -cover -remove",
        (
            f"sat -seq {args.depth} -set-assumes -set-init-zero -prove-asserts -verify "
            f"-timeout 180 -dump_vcd {out / 'counterexample.vcd'} -show-public"
        ),
    ]
    result = subprocess.run(
        ["yosys", "-Q", "-p", "; ".join(commands)],
        check=False,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        text=True,
    )
    (out / "proof.log").write_text(result.stdout)
    passed = result.returncode == 0 and "SUCCESS!" in result.stdout
    (out / "result.json").write_text(
        json.dumps(
            {
                "result": "bounded-pass" if passed else "failed",
                "depth": args.depth,
                "menu_commit": actual,
                "inputs": {
                    str(p): hashlib.sha256(p.read_bytes()).hexdigest() for p in inputs
                },
            },
            indent=2,
        )
        + "\n"
    )
    print(
        "boundary:",
        "bounded-pass" if passed else "failed; inspect counterexample/proof log",
        out,
    )
    return 0 if passed else 1


if __name__ == "__main__":
    raise SystemExit(main())
