#!/usr/bin/env python3
# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later
"""Exercise reset through the complete production scaler and pinned terminator.

This is a finite integration regression, not an exhaustive formal proof. A
physical-boundary failure remains a failure in the report and exit status.
"""

from __future__ import annotations

import argparse
from concurrent.futures import ThreadPoolExecutor, as_completed
import hashlib
import json
from pathlib import Path
import re
import subprocess

RESET_REGISTERS = {
    "avl": ("avl_read_sync", "avl_read_sync2", "avl_read_pulse"),
    "o": ("o_read", "o_readack_sync", "o_readack_sync2", "o_readack"),
}


def run(command, cwd, log):
    result = subprocess.run(
        command,
        cwd=cwd,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        check=False,
    )
    log.write_text(result.stdout)
    if result.returncode:
        raise RuntimeError(f"command failed: {command}; see {log}")
    return result.stdout


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def normalize_terminator(source):
    # Syntax normalization only, matching the existing boundary checker.
    for line in (
        "localparam BYTEENABLE_WIDTH = DATA_WIDTH/8;",
        "localparam ADDRESS_WITDH    = 32-$clog2(BYTEENABLE_WIDTH);",
    ):
        assert source.count(line) == 1
        source = source.replace(line, "")
    source = source.replace(
        "parameter     BURSTCOUNT_WIDTH = 8",
        "parameter BURSTCOUNT_WIDTH = 8,\nlocalparam BYTEENABLE_WIDTH = DATA_WIDTH/8,\n"
        "localparam ADDRESS_WITDH = 32-$clog2(BYTEENABLE_WIDTH)",
    )
    source = re.sub(r"\boutput\s+", "output logic ", source)
    source = re.sub(r"\bwire(\s+next_state_write;)", r"logic\1", source)
    declarations = re.findall(
        r"^reg \[(?:BURSTCOUNT_WIDTH|ADDRESS_WITDH)[^\n]+(?:write_burstcounter|write_burstcount_latch|write_address_latch)[^\n]+$",
        source,
        re.M,
    )
    assert len(declarations) == 3
    for declaration in declarations:
        source = source.replace(declaration, "")
    return source.replace(
        "reg  state_write", "\n".join(declarations) + "\nreg  state_write"
    )


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("menu_dir", type=Path)
    parser.add_argument("--artifacts-dir", type=Path, required=True)
    parser.add_argument(
        "--quick",
        action="store_true",
        help="run only the incident and outstanding-read cases",
    )
    args = parser.parse_args()
    root = Path(__file__).resolve().parents[2]
    rtl = root / "mister/platform/fpga/menu-vblank-latch"
    out = args.artifacts_dir.resolve()
    out.mkdir(parents=True, exist_ok=True)
    menu = args.menu_dir.resolve()
    pin = (rtl / "Menu_MiSTer.commit").read_text().strip()
    actual = subprocess.check_output(
        ["git", "-C", str(menu), "rev-parse", "HEAD"], text=True
    ).strip()
    if actual != pin:
        parser.error("Menu source must match the pinned revision")
    source_dir = out / "source"
    (source_dir / "sys").mkdir(parents=True, exist_ok=True)
    for name in ("ascal.vhd", "sys_top.v", "sysmem.sv", "f2sdram_safe_terminator.sv"):
        contents = subprocess.check_output(
            ["git", "-C", str(menu), "show", f"{pin}:sys/{name}"]
        )
        (source_dir / "sys" / name).write_bytes(contents)
    run(["git", "init", "--quiet"], source_dir, out / "source-init.log")
    patch = rtl / "Menu_MiSTer-vblank-latched-fbuf.patch"
    run(["git", "apply", "--check", str(patch)], source_dir, out / "patch-check.log")
    run(["git", "apply", str(patch)], source_dir, out / "patch.log")
    production = source_dir / "sys/ascal.vhd"
    source = production.read_text()
    baseline = source
    sys_top = (source_dir / "sys/sys_top.v").read_text()
    sysmem = (source_dir / "sys/sysmem.sv").read_text()
    for pattern in (r"\.run\s*\(1\)", r"\.reset_na\s*\(~reset_req\)"):
        assert re.search(pattern, sys_top)
    for fragment in (
        "vbuf_reset_0 <= reset_out;",
        "vbuf_reset_1 <= vbuf_reset_0;",
        ".rst_req_sync             (vbuf_reset_1)",
    ):
        assert fragment in sysmem
    for domain, registers in RESET_REGISTERS.items():
        matches = list(
            re.finditer(
                rf"IF {domain}_reset_na='0' THEN(.*?)ELSIF rising_edge\({domain}_clk\) THEN",
                source,
                re.S,
            )
        )
        match = next(m for m in matches if registers[0] + "<='0';" in m.group(1))
        for register in registers:
            assignment = f"\t\t\t{register}<='0';\n"
            assert match.group(1).count(assignment) == 1
            baseline = baseline.replace(assignment, "", 1)
    # Bind the mutation to exactly seven reset assignments, not a second RTL model.
    assert len(source.splitlines()) - len(baseline.splitlines()) == 7
    for retained in (
        "avl_return_credits",
        "avl_return_phase",
        "avl_read_accepted",
        "avl_read_i",
    ):
        branch = re.search(
            r"IF avl_reset_na='0' THEN(.*?)ELSIF rising_edge\(avl_clk\) THEN",
            source,
            re.S,
        ).group(1)
        assert not re.search(rf"\b{retained}\s*<=", branch)
    terminator = out / "terminator.sv"
    terminator.write_text(
        normalize_terminator(
            (source_dir / "sys/f2sdram_safe_terminator.sv").read_text()
        )
    )
    bench = rtl / "tb_mister_magik_scaler_reset.sv"
    results = []
    cases = [
        ("idle-high", {"scenario": 0}),
        ("idle-low", {"scenario": 0, "reset_after": 2}),
        ("no-reset", {"scenario": 15}),
    ]
    cases += [(f"stage-{i}", {"scenario": i}) for i in range(1, 18) if i != 15]
    if not args.quick:
        cases += [
            (
                f"boundary-stall-{stall}-lead-{lead}",
                {"scenario": 12, "release_stall": stall, "reset_lead": lead},
            )
            for stall in (0, 1, 2, 8, 129)
            for lead in (0, 1, 3, 7)
        ]
        cases += [
            (f"partial-{n}", {"scenario": 3, "partial_beats": n})
            for n in (1, 2, 63, 126, 127)
        ]
        cases += [
            (f"hold-{n}", {"scenario": 0, "reset_hold": n})
            for n in (200000, 5000000, 10000000, 20000000)
        ]
        cases += [
            (f"stop-{domain}-{scenario}", {"scenario": scenario, "stop_domain": domain})
            for domain in (1, 2)
            for scenario in (0, 2, 5, 9, 13)
        ]
        cases += [
            (f"repeated-{i}", {"scenario": i, "repeat_reset": 1}) for i in (0, 2, 3, 11)
        ]
        cases += [
            (
                f"clocks-{a}-{o}-{scenario}",
                {
                    "scenario": scenario,
                    "half_avl": a,
                    "half_output": o,
                    "clock_phase": 1701,
                    "return_gap": 3,
                },
            )
            for a, o in ((3367, 5000), (5000, 5000), (7000, 2500))
            for scenario in (0, 2, 3, 6, 10)
        ]
        cases += [
            (
                f"offset-{offset}-{scenario}",
                {"scenario": scenario, "reset_offset": offset},
            )
            for offset in (17, 2900, 7200)
            for scenario in (3, 6, 10, 13, 14)
        ]
    input_hashes = {
        str(p): digest(p)
        for p in (patch, production, terminator, bench, Path(__file__))
    }
    withdrawn = source.replace(
        "avl_read<=avl_read_i AND NOT avl_read_accepted;",
        "avl_read<=avl_read_i AND NOT avl_read_accepted\n"
        "\t\tWHEN avl_reset_na='0' OR avl_state=sREAD ELSE '0';",
    )
    withdrawn = withdrawn.replace(
        "avl_read_i,avl_read_accepted,avl_waitrequest);",
        "avl_read_i,avl_read_accepted,avl_waitrequest) AND\n"
        "\t\t\t\t(avl_reset_na='0' OR avl_state=sREAD);",
    )
    assert withdrawn != source
    for variant, contents in (
        ("baseline", baseline),
        ("withdrawn", withdrawn),
        ("candidate", source),
    ):
        work = out / variant
        work.mkdir(exist_ok=True)
        vhdl = work / "ascal.vhd"
        vhdl.write_text(contents)
        run(["ghdl", "-a", "--std=08", str(vhdl)], work, work / "analysis.log")
        result = subprocess.run(
            [
                "ghdl",
                "synth",
                "--std=08",
                "--out=verilog",
                "-gRAMBASE=00100000000000000000000000000000",
                "-gN_BURST=2048",
                "-gN_DW=128",
                "-gN_AW=28",
                "-gFRAC=8",
                "ascal",
            ],
            cwd=work,
            text=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            check=False,
        )
        (work / "synth.log").write_text(result.stderr)
        if result.returncode:
            raise RuntimeError(f"GHDL synthesis failed: {work}")
        netlist = work / "ascal.v"
        netlist.write_text(result.stdout)
        initialized = set(re.findall(r"initial\s+(\w+)\s*=", result.stdout))
        registers = re.findall(r"^  reg (?:\[[^\]]+\] )?(\w+);", result.stdout, re.M)
        (work / "powerup.vh").write_text(
            "initial begin\n"
            + "\n".join(
                f"dut.{register}=0;"
                for register in registers
                if register not in initialized
            )
            + "\nend\n"
        )
        run(
            [
                "iverilog",
                "-g2012",
                "-s",
                "tb_mister_magik_scaler_reset",
                "-I",
                str(work),
                "-o",
                str(work / "simulation"),
                str(netlist),
                str(terminator),
                str(bench),
            ],
            work,
            work / "compile.log",
        )

        def simulate(case_input, work=work, variant=variant):
            name, parameters = case_input
            case = work / name
            case.mkdir(exist_ok=True)
            command = ["vvp", str(work / "simulation"), "+vcd"]
            command += [f"+{key}={value}" for key, value in parameters.items()]
            result = subprocess.run(
                command,
                cwd=case,
                text=True,
                stdout=subprocess.PIPE,
                stderr=subprocess.STDOUT,
                check=False,
                timeout=180,
            )
            (case / "simulation.log").write_text(result.stdout)
            passed = result.returncode == 0 and "PASS " in result.stdout
            record = {
                "variant": variant,
                "case": name,
                "parameters": parameters,
                "passed": passed,
                "exit_code": result.returncode,
                "output": result.stdout.strip().splitlines()[-3:],
            }
            return record

        with ThreadPoolExecutor(max_workers=4) as executor:
            variant_cases = (
                [("stage-12", {"scenario": 12})] if variant == "withdrawn" else cases
            )
            futures = [executor.submit(simulate, case) for case in variant_cases]
            for future in as_completed(futures):
                record = future.result()
                results.append(record)
                print(
                    variant,
                    record["case"],
                    "PASS" if record["passed"] else "FAIL",
                    flush=True,
                )
                (out / "results.json").write_text(json.dumps(results, indent=2) + "\n")
    candidate_pass = all(r["passed"] for r in results if r["variant"] == "candidate")
    mutation_detected = any(
        not r["passed"] and any("UNCHARGED_READ" in line for line in r["output"])
        for r in results
        if r["variant"] == "baseline" and r["case"] == "idle-high"
    )
    withdrawal_detected = any(
        r["variant"] == "withdrawn"
        and not r["passed"]
        and any("PHYSICAL_LEDGER_MISMATCH" in line for line in r["output"])
        for r in results
    )
    candidate_pass = candidate_pass and withdrawal_detected
    report = {
        "result": "pass" if candidate_pass and mutation_detected else "failed",
        "scope": "complete-production-RTL finite reset integration matrix",
        "menu_commit": pin,
        "mutation_detected": mutation_detected,
        "withdrawal_detected": withdrawal_detected,
        "inputs": input_hashes,
        "baseline_ascal_sha256": hashlib.sha256(baseline.encode()).hexdigest(),
        "tools": {
            tool: subprocess.check_output(
                [tool, flag], text=True, stderr=subprocess.STDOUT
            ).splitlines()[0]
            for tool, flag in (("ghdl", "--version"), ("iverilog", "-V"))
        },
        "results": results,
    }
    (out / "report.json").write_text(json.dumps(report, indent=2) + "\n")
    return 0 if candidate_pass and mutation_detected else 1


if __name__ == "__main__":
    raise SystemExit(main())
