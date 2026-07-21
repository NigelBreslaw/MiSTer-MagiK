#!/usr/bin/env python3
# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

"""Apply and structurally verify the Menu latch and native CRT patches."""

from __future__ import annotations

import argparse
import re
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

COMMAND_PATTERNS = {
    "0x57": re.compile(r"(?:cmd|io_din\s*\[\s*7\s*:\s*0\s*\])\s*==\s*(?:8\s*'h|')57", re.I),
    "0x58": re.compile(r"(?:cmd|io_din\s*\[\s*7\s*:\s*0\s*\])\s*==\s*(?:8\s*'h|')58", re.I),
}


def fail(message: str) -> None:
    print(f"FPGA integration check failed: {message}", file=sys.stderr)
    raise SystemExit(1)


def qualified_menu_commit(root: Path) -> str:
    pin = root / "mister/platform/fpga/menu-vblank-latch/Menu_MiSTer.commit"
    try:
        contents = pin.read_text()
    except OSError as error:
        fail(f"cannot read qualified Menu source revision {pin}: {error}")
    if not re.fullmatch(r"[0-9a-f]{40}\n?", contents):
        fail(f"invalid qualified Menu source revision in {pin}: {contents!r}")
    commit = contents.rstrip("\n")
    return commit


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("menu_dir", type=Path)
    parser.add_argument("--allow-unpinned", action="store_true")
    args = parser.parse_args()

    root = Path(__file__).resolve().parents[2]
    pinned_menu_commit = qualified_menu_commit(root)
    menu = args.menu_dir.resolve()
    sys_top = menu / "sys/sys_top.v"
    qsf = menu / "menu.qsf"
    if not sys_top.is_file() or not qsf.is_file():
        fail(f"not a Menu_MiSTer checkout: {menu}")

    commit = subprocess.run(
        ["git", "-C", str(menu), "rev-parse", "HEAD"],
        check=True,
        text=True,
        capture_output=True,
    ).stdout.strip()
    if not args.allow_unpinned and commit != pinned_menu_commit:
        fail(f"Menu commit {commit} is not pinned {pinned_menu_commit}")

    conflicts: list[str] = []
    for path in sorted((menu / "sys").rglob("*")):
        if path.suffix.lower() not in {".v", ".sv", ".vh"}:
            continue
        text = path.read_text(errors="replace")
        for command, pattern in COMMAND_PATTERNS.items():
            if pattern.search(text):
                conflicts.append(f"{path.relative_to(menu)} uses {command}")
    if conflicts:
        fail("upstream opcode conflict: " + "; ".join(conflicts))

    patch = root / "mister/platform/fpga/menu-vblank-latch/Menu_MiSTer-vblank-latched-fbuf.patch"
    crt_patch = root / "mister/platform/fpga/menu-vblank-latch/Menu_MiSTer-native-crt.patch"
    rtl = root / "mister/platform/fpga/menu-vblank-latch/mister_magik_vblank_latch.sv"
    protocol = root / "mister/platform/fpga/menu-vblank-latch/mister_magik_latch_protocol.svh"
    crt_timing = root / "mister/platform/fpga/menu-vblank-latch/mister_magik_crt_timing.sv"
    crt_reader = root / "mister/platform/fpga/menu-vblank-latch/mister_magik_crt_reader.sv"
    with tempfile.TemporaryDirectory(prefix="mister-magik-fpga-integration-") as temporary:
        work = Path(temporary) / "Menu_MiSTer"
        shutil.copytree(menu, work, ignore=shutil.ignore_patterns(".git", "db", "output_files"))
        subprocess.run(["git", "apply", "--check", str(patch)], cwd=work, check=True)
        subprocess.run(["git", "apply", str(patch)], cwd=work, check=True)
        subprocess.run(
            ["git", "apply", "--ignore-space-change", "--check", str(crt_patch)],
            cwd=work,
            check=True,
        )
        subprocess.run(
            ["git", "apply", "--ignore-space-change", str(crt_patch)],
            cwd=work,
            check=True,
        )
        shutil.copy2(rtl, work / "sys/mister_magik_vblank_latch.sv")
        shutil.copy2(protocol, work / "sys/mister_magik_latch_protocol.svh")
        shutil.copy2(crt_timing, work / "sys/mister_magik_crt_timing.sv")
        shutil.copy2(crt_reader, work / "sys/mister_magik_crt_reader.sv")
        with (work / "menu.qsf").open("a") as output:
            output.write(
                "\nset_global_assignment -name SYSTEMVERILOG_FILE sys/mister_magik_vblank_latch.sv\n"
                "set_global_assignment -name SYSTEMVERILOG_FILE sys/mister_magik_crt_timing.sv\n"
                "set_global_assignment -name SYSTEMVERILOG_FILE sys/mister_magik_crt_reader.sv\n"
            )

        patched = (work / "sys/sys_top.v").read_text()
        required = (
            "mister_magik_vblank_latch magik_vblank_latch",
            ".crt_vblank(magik_crt_vblank)",
            ".apply_crt(magik_lfb_apply_crt)",
            ".cmd_start(io_uio && io_strobe && !has_cmd)",
            ".cmd_data(io_uio && io_strobe && has_cmd)",
            "if(magik_lfb_apply_hdmi)",
            "if(magik_lfb_apply_crt)",
            "if(magik_response_valid) io_dout_sys <= magik_response_data;",
            ".inclk({clk_vid, hdmi_clk_out, 2'b00})",
            "wire hdmi_base_tx_clk;",
            ".dataout(hdmi_base_tx_clk)",
            "magik_crtclk_ddr",
            "assign HDMI_TX_CLK = magik_crt_output ? magik_crt_tx_clk : hdmi_base_tx_clk;",
        )
        missing = [fragment for fragment in required if fragment not in patched]
        if missing:
            fail("patched integration is missing: " + ", ".join(missing))
        qsf_text = (work / "menu.qsf").read_text()
        for module in (
            "mister_magik_vblank_latch.sv",
            "mister_magik_crt_timing.sv",
            "mister_magik_crt_reader.sv",
        ):
            assignment = f"SYSTEMVERILOG_FILE sys/{module}"
            if qsf_text.count(assignment) != 1:
                fail(f"generated QSF must contain exactly one {module} assignment")

        menu_text = (work / "menu.sv").read_text()
        menu_required = (
            "mister_magik_crt_timing crt_timing",
            "mister_magik_crt_reader crt_reader",
            "assign CLK_VIDEO = clk_hdmi;",
            "assign MAGIK_CRT_CLK = clk_crt;",
            "assign MAGIK_CRT_DE = crt_de && crt_display_valid;",
        )
        menu_missing = [fragment for fragment in menu_required if fragment not in menu_text]
        if menu_missing:
            fail("patched Menu route is missing: " + ", ".join(menu_missing))
        if "cyclonev_clkselect magik_video_clk_sw" in menu_text:
            fail("CRT clock selector must live at the HDMI destination boundary")
        if "ddram ddr" in menu_text:
            fail("legacy DDR-clearing client remains in the patched Menu core")

    print(f"COVER LATCH-009 pinned Menu integration and opcode ownership ({commit})")
    print("FPGA latch integration check passed")


if __name__ == "__main__":
    main()
