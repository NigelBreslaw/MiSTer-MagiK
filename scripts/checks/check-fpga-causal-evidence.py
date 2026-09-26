#!/usr/bin/env python3
"""Prove and simulate the passive physical-boundary evidence observer."""

from __future__ import annotations
import argparse
import binascii
import hashlib
import json
import re
import shutil
import subprocess
from pathlib import Path


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--artifacts-dir", type=Path, required=True)
    args = parser.parse_args()
    root = Path(__file__).resolve().parents[2]
    rtl = root / "mister/platform/fpga/menu-vblank-latch"
    out = args.artifacts_dir.resolve()
    out.mkdir(parents=True, exist_ok=True)
    yosys = shutil.which("yosys")
    version = subprocess.check_output([yosys, "-V"], text=True).strip()
    if not version.startswith("Yosys 0.68 "):
        parser.error("pinned Yosys 0.68 is required")
    design = rtl / "mister_magik_scaler_causal_state.sv"
    formal = rtl / "mister_magik_scaler_causal_formal.sv"
    tb = rtl / "tb_mister_magik_scaler_causal_state.sv"

    def run(name, command):
        result = subprocess.run(
            command,
            cwd=out,
            text=True,
            check=False,
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
        )
        (out / (name + ".log")).write_text(result.stdout)
        if result.returncode:
            raise RuntimeError(f"{name} failed: {out / (name + '.log')}")
        return result.stdout

    run(
        "compile",
        [
            "iverilog",
            "-g2012",
            "-s",
            "tb_mister_magik_scaler_causal_state",
            "-o",
            str(out / "simulation"),
            str(design),
            str(tb),
        ],
    )
    faults = rtl / "tb_mister_magik_causal_faults.sv"
    run(
        "fault-compile",
        [
            "iverilog",
            "-g2012",
            "-s",
            "tb_mister_magik_causal_faults",
            "-o",
            str(out / "faults"),
            str(design),
            str(faults),
        ],
    )
    run("faults", ["vvp", str(out / "faults")])
    log = run("simulation", ["vvp", str(out / "simulation")])
    records = []
    for words in re.findall(r"^RECORD (.*)$", log, re.M):
        record = [int(v, 16) for v in words.split()]
        data = b"".join(v.to_bytes(2, "big") for v in [0x6A, 24, 4, *record[:-1]])
        if len(record) != 5 or binascii.crc_hqx(data, 0xFFFF) != record[-1]:
            raise RuntimeError("independent CRC check failed")
        records.append(record)
    if len(records) != 5 or "PASS: causal" not in log:
        raise RuntimeError("missing simulation records")
    prefix = f"read_verilog -formal -sv -DFORMAL {design} {formal}; hierarchy -check -top mister_magik_scaler_causal_formal; proc; flatten; clk2fflogic; opt_clean; chformal -cover -remove; "
    for name, options in [
        ("bounded", "-seq 32"),
        ("induction", "-seq 1 -tempinduct -maxsteps 32"),
    ]:
        run(
            name,
            [
                yosys,
                "-Q",
                "-p",
                prefix
                + f"sat {options} -set-init-zero -prove-asserts -verify -timeout 180 -dump_vcd {name}.vcd",
            ],
        )
    covers = {
        "cover_phase45": 120,
        "cover_drain": 24,
        "cover_invalid": 12,
        "cover_timeout": 300,
    }
    for name, depth in covers.items():
        log = run(
            name,
            [
                yosys,
                "-Q",
                "-p",
                prefix
                + f"chformal -assert -remove; sat -seq {depth} -set-init-zero -prove dut.{name} 0 -falsify -timeout 180 -dump_vcd {name}.vcd",
            ],
        )
        if "model found" not in log:
            raise RuntimeError("missing non-vacuity witness: " + name)
    for name, source, top in [
        ("candidate", design, "mister_magik_scaler_causal_state"),
        (
            "previous",
            rtl / "mister_magik_video_diagnostics_control.sv",
            "mister_magik_scaler_fetch_liveness_state",
        ),
    ]:
        run(
            name + "-lut6",
            [
                yosys,
                "-Q",
                "-p",
                f"read_verilog -sv -I{rtl} {source}; synth -top {top} -flatten -lut 6; stat; write_json {name}.json",
            ],
        )
    costs = {}
    for name in ("candidate", "previous"):
        from collections import Counter

        netlist = json.loads((out / (name + ".json")).read_text())
        cells = next(iter(netlist["modules"].values()))["cells"]
        costs[name] = dict(Counter(v["type"] for v in cells.values()))

    def total(c):
        return sum(value for key, value in c.items() if key != "$scopeinfo")

    if total(costs["candidate"]) >= total(costs["previous"]):
        raise RuntimeError("structural cost did not improve; reject before Quartus")
    report = {
        "costs_lut6": costs,
        "result": "pass",
        "tool": version,
        "records": records,
        "covers": covers,
        "inputs": {
            str(p.relative_to(root)): hashlib.sha256(p.read_bytes()).hexdigest()
            for p in (
                design,
                formal,
                tb,
                faults,
                rtl / "mister_magik_video_diagnostics_control.sv",
            )
        },
    }
    (out / "result.json").write_text(json.dumps(report, indent=2) + "\n")
    print(json.dumps(report, indent=2))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
