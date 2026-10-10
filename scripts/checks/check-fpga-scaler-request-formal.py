#!/usr/bin/env python3
# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later
"""Prove conservation in the production-derived request/acknowledgement transport.

The environment may schedule whenever the previous acknowledgement was consumed,
and may delay acceptance arbitrarily. Physical transactions and copy retirement
are checked separately by the complete-scaler integration regression.
"""

from __future__ import annotations
import argparse
import hashlib
import json
from pathlib import Path
import re
import subprocess


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("ascal", type=Path)
    parser.add_argument("--artifacts-dir", type=Path, required=True)
    args = parser.parse_args()
    source = args.ascal.read_text()
    out = args.artifacts_dir.resolve()
    out.mkdir(parents=True, exist_ok=True)
    root = Path(__file__).resolve().parents[2]
    wrapper = (
        root
        / "mister/platform/fpga/menu-vblank-latch/mister_magik_scaler_request_formal.sv"
    )
    version = subprocess.check_output(["yosys", "-V"], text=True).strip()
    if not version.startswith("Yosys 0.68 "):
        parser.error("pinned Yosys 0.68 is required")

    def assignment(name, expression):
        statement = f"{name}<={expression};"
        if source.count(statement) != 1:
            parser.error("production binding changed: " + statement)
        return statement

    def resets(domain, names):
        branches = re.findall(
            rf"IF {domain}_reset_na='0' THEN(.*?)ELSIF rising_edge\({domain}_clk\) THEN",
            source,
            re.S,
        )
        branch = next(b for b in branches if f"{names[0]}<='0';" in b)
        statements = []
        for name in names:
            statement = f"{name}<='0';"
            if branch.count(statement) != 1:
                parser.error("missing production reset: " + statement)
            statements.append(statement)
        return "\n".join(statements)

    output_reset = resets(
        "o", ("o_read_pre", "o_read", "o_readack_sync", "o_readack_sync2", "o_readack")
    )
    avalon_reset = resets(
        "avl", ("avl_readack", "avl_read_sync", "avl_read_sync2", "avl_read_pulse")
    )
    output_pipeline = "\n".join(
        assignment(n, e)
        for n, e in (
            ("o_read", "o_read_pre AND o_run"),
            ("o_readack_sync", "avl_readack"),
            ("o_readack_sync2", "o_readack_sync"),
            ("o_readack", "o_readack_sync XOR o_readack_sync2"),
        )
    )
    avalon_pipeline = "\n".join(
        assignment(n, e)
        for n, e in (
            ("avl_read_sync", "o_read"),
            ("avl_read_sync2", "avl_read_sync"),
            ("avl_read_pulse", "avl_read_sync XOR avl_read_sync2"),
        )
    )
    issue = assignment("o_read_pre", "NOT o_read_pre")
    accept = assignment("avl_readack", "NOT avl_readack")
    for domain in ("o", "avl"):
        if not re.search(
            rf"{domain}_reset_na<=\s*'0'\s+WHEN reset_na='0' ELSE '1' WHEN rising_edge\({domain}_clk\);",
            source,
        ):
            parser.error("production reset topology changed")
    vhdl = f"""LIBRARY ieee;
USE ieee.std_logic_1164.ALL;
ENTITY request_transport IS PORT (
 clk,reset_n,avl_step,o_step,schedule,accept_ready : IN std_logic;
 issue_event,receive_event,accept_event,ack_event : OUT std_logic;
 state : OUT std_logic_vector(12 DOWNTO 0)); END;
ARCHITECTURE rtl OF request_transport IS
 SIGNAL o_reset_na,avl_reset_na : std_logic;
 SIGNAL o_read_pre,o_read,o_readack_sync,o_readack_sync2,o_readack : std_logic;
 SIGNAL avl_readack,avl_read_sync,avl_read_sync2,avl_read_pulse : std_logic;
 SIGNAL waiting,pending : std_logic;
 CONSTANT o_run : std_logic := '1';
BEGIN
 -- Clock enables represent arbitrary domain edges, including indefinite stops.
 PROCESS(clk,reset_n) BEGIN
  IF reset_n='0' THEN o_reset_na<='0';avl_reset_na<='0';
  ELSIF rising_edge(clk) THEN
   IF o_step='1' THEN o_reset_na<='1';END IF;
   IF avl_step='1' THEN avl_reset_na<='1';END IF;
  END IF;
 END PROCESS;
 issue_event<=o_step AND o_reset_na AND schedule AND NOT waiting;
 receive_event<=avl_step AND avl_reset_na AND avl_read_pulse;
 accept_event<=avl_step AND avl_reset_na AND pending AND accept_ready;
 ack_event<=o_step AND o_reset_na AND o_readack;
 PROCESS(clk,o_reset_na) BEGIN
  IF o_reset_na='0' THEN
   {output_reset}
   waiting<='0';
  ELSIF rising_edge(clk) THEN
   IF o_step='1' THEN
    {output_pipeline}
    IF waiting='0' AND schedule='1' THEN {issue} waiting<='1';
    ELSIF waiting='1' AND o_readack='1' THEN waiting<='0';END IF;
   END IF;
  END IF;
 END PROCESS;
 PROCESS(clk,avl_reset_na) BEGIN
  IF avl_reset_na='0' THEN
   {avalon_reset}
   pending<='0';
  ELSIF rising_edge(clk) THEN
   IF avl_step='1' THEN
    {avalon_pipeline}
    IF avl_read_pulse='1' THEN pending<='1';END IF;
    IF pending='1' AND accept_ready='1' THEN {accept} pending<='0';END IF;
   END IF;
  END IF;
 END PROCESS;
 state<=avl_read_pulse & o_reset_na & avl_reset_na & waiting & pending &
  o_read_pre & o_read & avl_read_sync & avl_read_sync2 &
  avl_readack & o_readack_sync & o_readack_sync2 & o_readack;
END;
"""
    (out / "transport.vhd").write_text(vhdl)

    def run(name, command):
        result = subprocess.run(
            command,
            cwd=out,
            text=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
            check=False,
        )
        (out / f"{name}.log").write_text(result.stdout)
        if result.returncode:
            raise RuntimeError(f"{name} failed: {out}")
        return result.stdout

    run("analysis", ["ghdl", "-a", "--std=08", "transport.vhd"])
    netlist = run(
        "synthesis", ["ghdl", "synth", "--std=08", "--out=verilog", "request_transport"]
    )
    (out / "transport.v").write_text(netlist)
    prefix = f"read_verilog -formal transport.v; read_verilog -formal -sv {wrapper}; hierarchy -check -top request_formal; proc; flatten; clk2fflogic; opt_clean; chformal -cover -remove"
    base = run(
        "base",
        [
            "yosys",
            "-Q",
            "-p",
            prefix
            + "; sat -seq 64 -set-assumes -set-init-zero -prove-asserts -verify -timeout 180 -dump_vcd base-counterexample.vcd -show-public",
        ],
    )
    induction = run(
        "induction",
        [
            "yosys",
            "-Q",
            "-p",
            prefix
            + "; sat -seq 1 -tempinduct -set-assumes -set-init-zero -prove-asserts -verify -maxsteps 32 -timeout 180 -dump_vcd induction-counterexample.vcd -show-public",
        ],
    )
    if "SUCCESS!" not in base or "Induction step proven: SUCCESS!" not in induction:
        raise RuntimeError("proof did not complete")
    cover = run(
        "cover",
        [
            "yosys",
            "-Q",
            "-p",
            prefix
            + "; chformal -assert -remove; sat -seq 64 -set-assumes -set-init-zero -set-at 64 witnessed 1 -timeout 180 -dump_vcd cover.vcd -show-public",
        ],
    )
    if "SAT solving finished - model found" not in cover:
        raise RuntimeError(
            "required stopped-clock/reset/recovery witness is unreachable"
        )
    report = {
        "result": "pass",
        "scope": "production-derived request transport; abstract acceptance environment",
        "base_depth": 64,
        "induction_maxsteps": 32,
        "cover_depth": 64,
        "yosys": version,
        "inputs": {
            str(p): hashlib.sha256(p.read_bytes()).hexdigest()
            for p in (
                args.ascal,
                wrapper,
                Path(__file__),
                out / "transport.vhd",
                out / "transport.v",
            )
        },
    }
    (out / "report.json").write_text(json.dumps(report, indent=2) + "\n")
    print(json.dumps(report, indent=2))


if __name__ == "__main__":
    main()
