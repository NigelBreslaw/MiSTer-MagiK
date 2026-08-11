#!/usr/bin/env python3
# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

"""Apply and verify the production bridge integration against pinned Menu."""

from __future__ import annotations

import argparse
import hashlib
import re
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

COMMAND_PATTERNS = {
    "0x57": re.compile(r"(?:cmd|io_din\s*\[\s*7\s*:\s*0\s*\])\s*==\s*(?:8\s*'h|')57", re.I),
    "0x58": re.compile(r"(?:cmd|io_din\s*\[\s*7\s*:\s*0\s*\])\s*==\s*(?:8\s*'h|')58", re.I),
    "0x59": re.compile(r"(?:cmd|io_din\s*\[\s*7\s*:\s*0\s*\])\s*==\s*(?:8\s*'h|')59", re.I),
    "0x5a": re.compile(r"(?:cmd|io_din\s*\[\s*7\s*:\s*0\s*\])\s*==\s*(?:8\s*'h|')5a", re.I),
    "0x5b": re.compile(r"(?:cmd|io_din\s*\[\s*7\s*:\s*0\s*\])\s*==\s*(?:8\s*'h|')5b", re.I),
    "0x5c": re.compile(r"(?:cmd|io_din\s*\[\s*7\s*:\s*0\s*\])\s*==\s*(?:8\s*'h|')5c", re.I),
    "0x5d": re.compile(r"(?:cmd|io_din\s*\[\s*7\s*:\s*0\s*\])\s*==\s*(?:8\s*'h|')5d", re.I),
    "0x5e": re.compile(r"(?:cmd|io_din\s*\[\s*7\s*:\s*0\s*\])\s*==\s*(?:8\s*'h|')5e", re.I),
    "0x5f": re.compile(r"(?:cmd|io_din\s*\[\s*7\s*:\s*0\s*\])\s*==\s*(?:8\s*'h|')5f", re.I),
}

IMMUTABLE_LATCH_SHA256 = {
    "mister_magik_vblank_latch.sv": "47def40bc8b064373efa328a56ab0396272855a2190f17d700f27b8a29382090",
    "mister_magik_latch_sys_top_bridge.sv": "5960883a0f8740ffc18fcd63ce8c99da9a9819bcbc903ca119bfc66810fd68e9",
    "mister_magik_latch_protocol.svh": "bc26dff578940790a70e379718f8f1b8eda7122efd267e0e5e7cc244f1347a7b",
    "latch-protocol.json": "69eef7979ad235c49989870b82534d187c9d97da40feb6e7647fd8e62adbec54",
}

BRIDGE_MAPPING = """mister_magik_latch_sys_top_bridge magik_latch_bridge
(
\t.clk_sys(clk_sys),
\t.hdmi_vbl(hdmi_vbl),
\t.io_uio(io_uio),
\t.io_strobe(io_strobe),
\t.io_din(io_din),
\t.active_lfb_en(LFB_EN),
\t.active_lfb_base(LFB_BASE),
\t.active_lfb_width(LFB_WIDTH),
\t.active_lfb_height(LFB_HEIGHT),
\t.active_lfb_stride(LFB_STRIDE),
\t.response_valid(magik_response_valid),
\t.response_data(magik_response_data),
\t.apply(),
\t.apply_accepted(magik_lfb_apply_accepted),
\t.legacy_write(),
\t.active_word_index(),
\t.route_en(magik_lfb_en),
\t.route_flt(magik_lfb_flt),
\t.route_fmt(magik_lfb_fmt),
\t.route_width(magik_lfb_width),
\t.route_height(magik_lfb_height),
\t.route_hmin(magik_lfb_hmin),
\t.route_hmax(magik_lfb_hmax),
\t.route_vmin(magik_lfb_vmin),
\t.route_vmax(magik_lfb_vmax),
\t.route_base(magik_lfb_base),
\t.route_stride(magik_lfb_stride),
\t.pending(magik_lfb_pending),
\t.pending_seq(magik_lfb_pending_seq),
\t.active_seq(magik_lfb_active_seq),
\t.post_count(magik_lfb_post_count),
\t.flip_count(magik_lfb_flip_count),
\t.drop_count(magik_lfb_drop_count),
\t.reject_count(magik_lfb_reject_count),
\t.active_route_epoch(magik_lfb_active_route_epoch)
);"""

APPLY_BUNDLE = """if(magik_lfb_apply_accepted) begin
\t\tLFB_EN     <= magik_lfb_en;
\t\tLFB_FLT    <= magik_lfb_flt;
\t\tLFB_FMT    <= magik_lfb_fmt;
\t\tLFB_WIDTH  <= magik_lfb_width;
\t\tLFB_HEIGHT <= magik_lfb_height;
\t\tLFB_HMIN   <= magik_lfb_hmin;
\t\tLFB_HMAX   <= magik_lfb_hmax;
\t\tLFB_VMIN   <= magik_lfb_vmin;
\t\tLFB_VMAX   <= magik_lfb_vmax;
\t\tLFB_BASE   <= magik_lfb_base;
\t\tLFB_STRIDE <= magik_lfb_stride;
\tend"""


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
    return contents.rstrip("\n")


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("menu_dir", type=Path)
    parser.add_argument("--allow-unpinned", action="store_true")
    parser.add_argument(
        "--simulate",
        action="store_true",
        help="drive the exact production command/strobe bridge and latch RTL",
    )
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

    source_dir = root / "mister/platform/fpga/menu-vblank-latch"
    patch = source_dir / "Menu_MiSTer-vblank-latched-fbuf.patch"
    rtl = source_dir / "mister_magik_vblank_latch.sv"
    bridge = source_dir / "mister_magik_latch_sys_top_bridge.sv"
    bootstrap_black = source_dir / "mister_magik_bootstrap_black.sv"
    protocol = source_dir / "mister_magik_latch_protocol.svh"
    diagnostics_control = source_dir / "mister_magik_video_diagnostics_control.sv"
    diagnostics_avalon = source_dir / "mister_magik_video_diagnostics_avalon.sv"
    diagnostics_output = source_dir / "mister_magik_video_diagnostics_output.sv"
    diagnostics_protocol = source_dir / "mister_magik_video_diagnostics_protocol.svh"
    diagnostics_sdc = source_dir / "mister_magik_video_diagnostics.sdc"
    integration_tb = source_dir / "tb_mister_magik_sys_top_integration.sv"
    control_source = diagnostics_control.read_text()
    avalon_source = diagnostics_avalon.read_text()
    output_source = diagnostics_output.read_text()
    if re.search(r"\bvbuf_(?:read|write)data\b", avalon_source):
        fail("passive Avalon diagnostics source must not expose framebuffer data")
    sync_assignment = "SYNCHRONIZER_IDENTIFICATION FORCED_IF_ASYNCHRONOUS"
    if "ASYNC_REG" in avalon_source or avalon_source.count(sync_assignment) != 5:
        fail("passive Avalon diagnostics synchronizers are not explicitly identified")
    if "ASYNC_REG" in output_source or output_source.count(sync_assignment) != 8:
        fail("final HDMI diagnostics synchronizers are not explicitly identified")
    if "ASYNC_REG" in control_source or control_source.count(sync_assignment) != 9:
        fail("control diagnostics synchronizers are not explicitly identified")
    control_cdc_bindings = {
        "hdmi_vbl": "control_vbl_meta <= hdmi_vbl;",
        "reset_req": "control_reset_req_meta <= reset_req;",
        "reset_out": "control_reset_out_meta <= reset_out;",
        "pll_adjust_locked": "control_pll_lock_meta <= pll_adjust_locked;",
    }
    for raw_name, binding in control_cdc_bindings.items():
        if binding not in control_source or len(re.findall(rf"\b{raw_name}\b", control_source)) != 2:
            fail(f"control diagnostics consumes raw asynchronous {raw_name} outside its first stage")
    if "cfg_done_meta" in control_source or "cfg_done_sys" in control_source:
        fail("clk_sys cfg_done was unnecessarily synchronized in control diagnostics")
    diagnostics_sdc_text = diagnostics_sdc.read_text()
    if "get_registers -nowarn -hierarchical" in diagnostics_sdc_text:
        fail("diagnostic SDC uses unsupported Quartus get_registers syntax")
    if "get_registers -nowarn -no_duplicates" not in diagnostics_sdc_text:
        fail("diagnostic SDC does not use exact non-duplicated register collections")
    if diagnostics_sdc_text.count("set_net_delay -max") != 1:
        fail("diagnostic bundled-data net-delay constraint is missing")
    if diagnostics_sdc_text.count("set_max_skew -from") != 1:
        fail("diagnostic bundled-data skew constraint is missing")
    if diagnostics_sdc_text.count("-exclude {ccpp}") != 1:
        fail("diagnostic skew constraint does not suppress inapplicable CCPP analysis")
    if "fault_burstcount*" in diagnostics_sdc_text:
        fail("diagnostic SDC requires the constant-folded burstcount payload register")
    if "reference_flags*" in diagnostics_sdc_text:
        fail("diagnostic SDC requires the constant-folded reference-flags register")
    if diagnostics_sdc_text.count("magik_require_registers") < 8:
        fail("diagnostic CDC constraints do not reject empty node collections")
    diagnostic_hierarchies = (
        "mister_magik_video_diagnostics_control:magik_video_diagnostics|",
        "mister_magik_video_diagnostics_avalon:magik_video_diagnostics_avalon|",
        "mister_magik_video_diagnostics_output:magik_video_diagnostics_output|",
    )
    for hierarchy in diagnostic_hierarchies:
        if hierarchy not in diagnostics_sdc_text:
            fail(f"diagnostic SDC does not use synthesized hierarchy {hierarchy}")
    with tempfile.TemporaryDirectory(prefix="mister-magik-fpga-integration-") as temporary:
        work = Path(temporary) / "Menu_MiSTer"
        shutil.copytree(menu, work, ignore=shutil.ignore_patterns(".git", "db", "output_files"))
        subprocess.run(["git", "apply", "--check", str(patch)], cwd=work, check=True)
        subprocess.run(["git", "apply", str(patch)], cwd=work, check=True)
        sys_top_sdc = work / "sys/sys_top.sdc"
        sdc_bytes = sys_top_sdc.read_bytes()
        clock_group = b"set_clock_groups -exclusive"
        if sdc_bytes.count(clock_group) != 1:
            fail("pinned Menu clock-group constraint changed unexpectedly")
        sys_top_sdc.write_bytes(
            sdc_bytes.replace(clock_group, b"set_clock_groups -asynchronous")
        )
        if sys_top_sdc.read_bytes().count(b"set_clock_groups -asynchronous") != 1:
            fail("diagnostic clock groups were not marked asynchronous")
        for name, expected in IMMUTABLE_LATCH_SHA256.items():
            actual = hashlib.sha256((source_dir / name).read_bytes()).hexdigest()
            if actual != expected:
                fail(f"immutable latch source changed: {name} expected {expected}, got {actual}")
        for source in (
            rtl,
            bridge,
            bootstrap_black,
            protocol,
            diagnostics_control,
            diagnostics_avalon,
            diagnostics_output,
            diagnostics_protocol,
            diagnostics_sdc,
        ):
            shutil.copy2(source, work / "sys" / source.name)
        with (work / "menu.qsf").open("a") as output:
            output.write(
                "\nset_global_assignment -name SYSTEMVERILOG_FILE "
                "sys/mister_magik_vblank_latch.sv\n"
                "set_global_assignment -name SYSTEMVERILOG_FILE "
                "sys/mister_magik_latch_sys_top_bridge.sv\n"
                "set_global_assignment -name SYSTEMVERILOG_FILE "
                "sys/mister_magik_bootstrap_black.sv\n"
                "set_global_assignment -name SYSTEMVERILOG_FILE "
                "sys/mister_magik_video_diagnostics_control.sv\n"
                "set_global_assignment -name SYSTEMVERILOG_FILE "
                "sys/mister_magik_video_diagnostics_avalon.sv\n"
                "set_global_assignment -name SYSTEMVERILOG_FILE "
                "sys/mister_magik_video_diagnostics_output.sv\n"
                "set_global_assignment -name SDC_FILE "
                "sys/mister_magik_video_diagnostics.sdc\n"
            )

        patched = (work / "sys/sys_top.v").read_text()
        required_counts = {
            BRIDGE_MAPPING: 1,
            APPLY_BUNDLE: 1,
            "if(magik_response_valid) io_dout_sys <= magik_response_data;": 2,
            "mister_magik_video_diagnostics_control magik_video_diagnostics": 1,
            "mister_magik_video_diagnostics_avalon magik_video_diagnostics_avalon": 1,
            "mister_magik_video_diagnostics_output magik_video_diagnostics_output": 1,
            "if(magik_diag_response_valid) io_dout_sys <= magik_diag_response_data;\n"
            "\t\t\tif(magik_response_valid) io_dout_sys <= magik_response_data;": 2,
        }
        mismatches = [
            f"{fragment.splitlines()[0]!r} expected {expected}, found {patched.count(fragment)}"
            for fragment, expected in required_counts.items()
            if patched.count(fragment) != expected
        ]
        if mismatches:
            fail("patched production bridge binding mismatch: " + "; ".join(mismatches))
        if re.search(
            r"\b(?:LFB_|FB_|hdmi_out_|vbuf_|reset_req)[A-Za-z0-9_]*\s*"
            r"(?:<=|=)\s*magik_diag_",
            patched,
        ):
            fail("diagnostic output reaches a functional datapath assignment")

        avalon_binding = re.search(
            r"mister_magik_video_diagnostics_avalon\s+"
            r"magik_video_diagnostics_avalon\s*\((.*?)\);",
            patched,
            re.S,
        )
        if avalon_binding is None:
            fail("missing passive Avalon diagnostics binding")
        if re.search(r"\.vbuf_(?:read|write)data\s*\(", avalon_binding.group(1)):
            fail("passive Avalon diagnostics must not tap framebuffer data")

        output_binding = re.search(
            r"mister_magik_video_diagnostics_output\s+"
            r"magik_video_diagnostics_output\s*\((.*?)\);",
            patched,
            re.S,
        )
        if output_binding is None:
            fail("missing final registered HDMI diagnostics binding")
        for required_tap in (
            ".hdmi_out_d(hdmi_out_d)",
            ".hdmi_out_de(hdmi_out_de)",
            ".hdmi_out_hs(hdmi_out_hs)",
            ".hdmi_out_vs(hdmi_out_vs)",
        ):
            if output_binding.group(1).count(required_tap) != 1:
                fail(f"final HDMI diagnostics tap mismatch: {required_tap}")

        video_paths = {
            "native black to shared scanline stage": re.compile(
                r"mister_magik_bootstrap_black\s+magik_bootstrap_black\s*"
                r"\(.*?\.rgb_out\(magik_native_data\).*?"
                r"\.de_out\(magik_native_de\).*?"
                r"\.hs_out\(magik_native_hs\).*?"
                r"\.vs_out\(magik_native_vs\).*?\);\s*"
                r"scanlines\s*#\(0\)\s+VGA_scanlines\s*\(.*?"
                r"\.din\(magik_native_data\).*?"
                r"\.hs_in\(magik_native_hs\).*?"
                r"\.vs_in\(magik_native_vs\).*?"
                r"\.de_in\(magik_native_de\).*?"
                r"\.dout\(vga_data_sl\)",
                re.S,
            ),
            "shared black stage to HDMI scaler": re.compile(
                r"\.i_r\s*\(hr_out\).*?\.i_g\s*\(hg_out\).*?"
                r"\.i_b\s*\(hb_out\)",
                re.S,
            ),
            "shared black stage HDMI assignments": re.compile(
                r"assign\s+hr_out\s*=\s*vga_data_sl\[23:16\];\s*"
                r"assign\s+hg_out\s*=\s*vga_data_sl\[15:8\];\s*"
                r"assign\s+hb_out\s*=\s*vga_data_sl\[7:0\];"
            ),
            "HDMI downstream OSD composition": re.compile(
                r"shadowmask\s+HDMI_shadowmask\s*\(.*?"
                r"\.din\(dis_output\s*\?\s*24'd0\s*:\s*hdmi_data\).*?"
                r"\.dout\(hdmi_data_mask\).*?\);\s*"
                r".*?osd\s+hdmi_osd\s*\(.*?"
                r"\.din\(hdmi_data_mask\).*?"
                r"\.dout\(hdmi_data_osd\)",
                re.S,
            ),
            "analog downstream OSD composition": re.compile(
                r"osd\s+vga_osd\s*\(.*?"
                r"\.din\(vga_data_sl\).*?"
                r"\.hs_in\(vga_hs_sl\).*?"
                r"\.vs_in\(vga_vs_sl\).*?"
                r"\.de_in\(vga_de_sl\).*?"
                r"\.dout\(vga_data_osd\)",
                re.S,
            ),
        }
        missing_video_paths = [
            label for label, pattern in video_paths.items() if not pattern.search(patched)
        ]
        if missing_video_paths:
            fail(
                "MagiK native-black video path mismatch: "
                + ", ".join(missing_video_paths)
            )

        qsf_text = (work / "menu.qsf").read_text()
        assignments = (
            "SYSTEMVERILOG_FILE sys/mister_magik_vblank_latch.sv",
            "SYSTEMVERILOG_FILE sys/mister_magik_latch_sys_top_bridge.sv",
            "SYSTEMVERILOG_FILE sys/mister_magik_bootstrap_black.sv",
            "SYSTEMVERILOG_FILE sys/mister_magik_video_diagnostics_control.sv",
            "SYSTEMVERILOG_FILE sys/mister_magik_video_diagnostics_avalon.sv",
            "SYSTEMVERILOG_FILE sys/mister_magik_video_diagnostics_output.sv",
            "SDC_FILE sys/mister_magik_video_diagnostics.sdc",
        )
        bad_assignments = [
            assignment for assignment in assignments if qsf_text.count(assignment) != 1
        ]
        if bad_assignments:
            fail("generated QSF assignment mismatch: " + ", ".join(bad_assignments))

        if args.simulate:
            simulation = Path(temporary) / "sys-top-integration.vvp"
            subprocess.run(
                [
                    "iverilog",
                    "-g2012",
                    "-Wall",
                    "-Wimplicit",
                    "-I",
                    str(source_dir),
                    "-s",
                    "tb_mister_magik_sys_top_integration",
                    "-o",
                    str(simulation),
                    str(rtl),
                    str(bridge),
                    str(integration_tb),
                ],
                check=True,
            )
            subprocess.run(["vvp", str(simulation)], check=True)

    print(f"COVER LATCH-009 pinned Menu production bridge and opcode ownership ({commit})")
    print("FPGA latch integration check passed")


if __name__ == "__main__":
    main()
