#!/usr/bin/env python3
"""Apply and structurally verify the latch-only Menu integration patch."""

from __future__ import annotations

import argparse
import re
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

PINNED_MENU_COMMIT = "3c3634c0105d78f27aeba66b38966c50dbc42c9b"
COMMAND_PATTERNS = {
    "0x57": re.compile(r"(?:cmd|io_din\s*\[\s*7\s*:\s*0\s*\])\s*==\s*(?:8\s*'h|')57", re.I),
    "0x58": re.compile(r"(?:cmd|io_din\s*\[\s*7\s*:\s*0\s*\])\s*==\s*(?:8\s*'h|')58", re.I),
}


def fail(message: str) -> None:
    print(f"FPGA integration check failed: {message}", file=sys.stderr)
    raise SystemExit(1)


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("menu_dir", type=Path)
    parser.add_argument("--allow-unpinned", action="store_true")
    args = parser.parse_args()

    root = Path(__file__).resolve().parents[1]
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
    if not args.allow_unpinned and commit != PINNED_MENU_COMMIT:
        fail(f"Menu commit {commit} is not pinned {PINNED_MENU_COMMIT}")

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

    patch = root / "fpga/menu-vblank-latch/Menu_MiSTer-vblank-latched-fbuf.patch"
    rtl = root / "fpga/menu-vblank-latch/mister_magik_vblank_latch.sv"
    with tempfile.TemporaryDirectory(prefix="mister-magik-fpga-integration-") as temporary:
        work = Path(temporary) / "Menu_MiSTer"
        shutil.copytree(menu, work, ignore=shutil.ignore_patterns(".git", "db", "output_files"))
        subprocess.run(["git", "apply", "--check", str(patch)], cwd=work, check=True)
        subprocess.run(["git", "apply", str(patch)], cwd=work, check=True)
        shutil.copy2(rtl, work / "sys/mister_magik_vblank_latch.sv")
        with (work / "menu.qsf").open("a") as output:
            output.write("\nset_global_assignment -name SYSTEMVERILOG_FILE sys/mister_magik_vblank_latch.sv\n")

        patched = (work / "sys/sys_top.v").read_text()
        required = (
            "mister_magik_vblank_latch magik_vblank_latch",
            ".cmd_start(io_uio && io_strobe && !has_cmd)",
            ".cmd_data(io_uio && io_strobe && has_cmd)",
            "if(magik_lfb_apply)",
            "if(magik_response_valid) io_dout_sys <= magik_response_data;",
        )
        missing = [fragment for fragment in required if fragment not in patched]
        if missing:
            fail("patched integration is missing: " + ", ".join(missing))
        qsf_text = (work / "menu.qsf").read_text()
        assignment = "SYSTEMVERILOG_FILE sys/mister_magik_vblank_latch.sv"
        if qsf_text.count(assignment) != 1:
            fail("generated QSF must contain exactly one latch RTL assignment")

    print(f"COVER LATCH-009 pinned Menu integration and opcode ownership ({commit})")
    print("FPGA latch integration check passed")


if __name__ == "__main__":
    main()
