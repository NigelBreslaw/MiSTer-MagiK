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
    "0x60": re.compile(r"(?:cmd|io_din\s*\[\s*7\s*:\s*0\s*\])\s*==\s*(?:8\s*'h|')60", re.I),
    "0x61": re.compile(r"(?:cmd|io_din\s*\[\s*7\s*:\s*0\s*\])\s*==\s*(?:8\s*'h|')61", re.I),
    "0x62": re.compile(r"(?:cmd|io_din\s*\[\s*7\s*:\s*0\s*\])\s*==\s*(?:8\s*'h|')62", re.I),
    "0x63": re.compile(r"(?:cmd|io_din\s*\[\s*7\s*:\s*0\s*\])\s*==\s*(?:8\s*'h|')63", re.I),
    "0x64": re.compile(r"(?:cmd|io_din\s*\[\s*7\s*:\s*0\s*\])\s*==\s*(?:8\s*'h|')64", re.I),
    "0x65": re.compile(r"(?:cmd|io_din\s*\[\s*7\s*:\s*0\s*\])\s*==\s*(?:8\s*'h|')65", re.I),
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
    timing_report = source_dir / "report_top_timing.tcl"
    integration_tb = source_dir / "tb_mister_magik_sys_top_integration.sv"
    control_source = diagnostics_control.read_text()
    avalon_source = diagnostics_avalon.read_text()
    output_source = diagnostics_output.read_text()
    if re.search(r"\bmodule\b", avalon_source) or re.search(r"\bmodule\b", output_source):
        fail("retired native-domain diagnostic source unexpectedly defines logic")
    compiled_diagnostics = control_source + avalon_source + output_source
    for retired_fragment in (
        "snapshot_payload",
        "snapshot_request",
        "expected_route_epoch",
        "expected_active_seq",
        "expected_base",
        "diagnostic_generation",
        "route_context",
        "fault_toggle",
        "heartbeat_toggle",
        "vbuf_address",
        "reset_req",
        "cfg_done",
        "altsyncram",
        "M10K",
    ):
        if retired_fragment in compiled_diagnostics:
            fail(f"retired wide diagnostic fragment remains in lock recorder: {retired_fragment}")
    sync_assignment = "SYNCHRONIZER_IDENTIFICATION FORCED_IF_ASYNCHRONOUS"
    synchronizer_stages = (
        ("control_pll_lock_meta", "FORCED"),
        ("control_pll_lock_sys", "FORCED_IF_ASYNCHRONOUS"),
        ("output_no_de_meta", "FORCED"),
        ("output_no_de_sys", "FORCED_IF_ASYNCHRONOUS"),
        ("output_black_direct_meta", "FORCED"),
        ("output_black_direct_sys", "FORCED_IF_ASYNCHRONOUS"),
        ("output_black_scaled_meta", "FORCED"),
        ("output_black_scaled_sys", "FORCED_IF_ASYNCHRONOUS"),
        ("output_black_mixed_meta", "FORCED"),
        ("output_black_mixed_sys", "FORCED_IF_ASYNCHRONOUS"),
        ("output_de_has_nonzero_meta", "FORCED"),
        ("output_de_has_nonzero_sys", "FORCED_IF_ASYNCHRONOUS"),
    )
    if (
        "ASYNC_REG" in control_source
        or control_source.count(sync_assignment) != 6
    ):
        fail("HDMI evidence synchronizers are not exactly identified")
    for stage, assignment in synchronizer_stages:
        declaration = (
            f'(* altera_attribute = "-name SYNCHRONIZER_IDENTIFICATION {assignment}" *)\n'
            f"\treg {stage} = 1'b0;"
        )
        if control_source.count(declaration) != 1:
            fail(f"HDMI evidence synchronizer stage is not exact: {stage}")
    if (
        control_source.count("control_pll_lock_meta <= hdmi_pll_locked;") != 1
        or len(re.findall(r"\bhdmi_pll_locked\b", control_source)) != 2
    ):
        fail("lock recorder consumes raw HDMI PLL status outside its first stage")
    synchronizer_bindings = {
        "control PLL first stage": (
            control_source,
            "control_pll_lock_meta <= hdmi_pll_locked;",
        ),
        "control PLL second stage": (
            control_source,
            "control_pll_lock_sys <= control_pll_lock_meta;",
        ),
        "no-DE first stage": (
            control_source,
            "output_no_de_meta <= output_no_de_toggle;",
        ),
        "no-DE second stage": (
            control_source,
            "output_no_de_sys <= output_no_de_meta;",
        ),
        "direct-black first stage": (
            control_source,
            "output_black_direct_meta <= output_black_direct_toggle;",
        ),
        "direct-black second stage": (
            control_source,
            "output_black_direct_sys <= output_black_direct_meta;",
        ),
        "scaled-black first stage": (
            control_source,
            "output_black_scaled_meta <= output_black_scaled_toggle;",
        ),
        "scaled-black second stage": (
            control_source,
            "output_black_scaled_sys <= output_black_scaled_meta;",
        ),
        "mixed-black first stage": (
            control_source,
            "output_black_mixed_meta <= output_black_mixed_toggle;",
        ),
        "mixed-black second stage": (
            control_source,
            "output_black_mixed_sys <= output_black_mixed_meta;",
        ),
        "nonzero first stage": (
            control_source,
            "output_de_has_nonzero_meta <= output_de_has_nonzero_toggle;",
        ),
        "nonzero second stage": (
            control_source,
            "output_de_has_nonzero_sys <= output_de_has_nonzero_meta;",
        ),
    }
    for label, (source, binding) in synchronizer_bindings.items():
        if source.count(binding) != 1:
            fail(f"{label} binding mismatch")
    if control_source.count("module mister_magik_hdmi_lock_evidence") != 1:
        fail("HDMI lock and output evidence module is missing or ambiguous")
    module_match = re.search(
        r"module\s+mister_magik_hdmi_lock_evidence\s*\((.*?)\);",
        control_source,
        re.S,
    )
    if module_match is None:
        fail("HDMI evidence module interface is missing")
    module_ports = re.findall(
        r"\b(?:input|output)\s+(?:wire|reg)\s+(?:\[[^\]]+\]\s+)?"
        r"([A-Za-z_][A-Za-z0-9_]*)",
        module_match.group(1),
    )
    expected_module_ports = [
        "clk_sys",
        "hdmi_tx_clk",
        "io_uio",
        "io_strobe",
        "io_din",
        "hdmi_pll_locked",
        "hdmi_out_vs",
        "hdmi_out_de",
        "hdmi_out_d",
        "hdmi_out_direct",
        "response_valid",
        "response_data",
    ]
    if module_ports != expected_module_ports:
        fail("HDMI evidence module interface is not the exact passive allowlist")
    required_activity_fragments = (
        "wire output_sample_nonzero = hdmi_out_de && (|hdmi_out_d);",
        "if(!output_frame_saw_de_now)",
        "else if(output_frame_saw_nonzero_now)",
        "output_no_de_toggle <= !output_no_de_toggle;",
        "output_black_direct_toggle <= !output_black_direct_toggle;",
        "output_black_scaled_toggle <= !output_black_scaled_toggle;",
        "output_black_mixed_toggle <= !output_black_mixed_toggle;",
        "output_de_has_nonzero_toggle <= !output_de_has_nonzero_toggle;",
        "reg [3:0] output_no_de_count = 4'd0;",
        "reg [3:0] output_black_direct_count = 4'd0;",
        "reg [3:0] output_black_scaled_count = 4'd0;",
        "reg [3:0] output_black_mixed_count = 4'd0;",
        "reg [3:0] output_de_has_nonzero_count = 4'd0;",
        "wire output_no_de_event = output_no_de_sys != output_no_de_count[0];",
        "wire activity_start = io_din[7:0] == MAGIK_UIO_GET_HDMI_OUTPUT_ACTIVITY;",
        "tx_crc <= MAGIK_HDMI_OUTPUT_ACTIVITY_HEADER_CRC;",
    )
    for fragment in required_activity_fragments:
        if control_source.count(fragment) != 1:
            fail(f"final-output evidence behavior is missing or ambiguous: {fragment}")
    if re.search(
        r"\b(?:LFB_|FB_|vbuf_|reset_req|cfg_done)[A-Za-z0-9_]*\s*(?:<=|=)",
        control_source,
    ):
        fail("HDMI evidence module drives a functional video or control signal")
    for redundant_register in (
        "output_no_de_previous",
        "output_black_direct_previous",
        "output_black_scaled_previous",
        "output_black_mixed_previous",
        "output_de_has_nonzero_previous",
        "snapshot_output_nonzero_count",
    ):
        if redundant_register in control_source:
            fail(f"HDMI activity recorder regained redundant state: {redundant_register}")
    diagnostics_sdc_text = diagnostics_sdc.read_text()
    timing_report_text = timing_report.read_text()
    unconstrained_report = (
        "report_ucp \\\n"
        "\t-file output_files/menu.unconstrained-paths.rpt"
    )
    if timing_report_text.count(unconstrained_report) != 1:
        fail("full unconstrained-path timing report is missing or ambiguous")
    if "get_pins -nowarn -no_duplicates" not in diagnostics_sdc_text:
        fail("diagnostic SDC does not constrain direct register data pins")
    pll_lock_false_path = "set_false_path -to $magik_hdmi_lock_meta_pin"
    timing_commands = re.findall(
        r"(?m)^\s*(set_[A-Za-z0-9_]+\b[^\n]*)$", diagnostics_sdc_text
    )
    if (
        timing_commands != [pll_lock_false_path]
        or diagnostics_sdc_text.count("control_pll_lock_meta}") != 1
    ):
        fail("HDMI lock SDC contains more than the sole approved first-stage false path")
    if diagnostics_sdc_text.count("magik_require_data_pin") != 2:
        fail("HDMI lock constraint does not require one exact first-stage data pin")
    if (
        "foreach suffix [list d asdata sdata]" not in diagnostics_sdc_text
        or '"${register_pattern}|${suffix}"' not in diagnostics_sdc_text
    ):
        fail("HDMI lock constraint omits a legal direct register data pin")
    if diagnostics_sdc_text.count(
        "mister_magik_hdmi_lock_evidence:magik_hdmi_lock_evidence|"
    ) != 1:
        fail("HDMI lock SDC does not use the exact synthesized hierarchy")
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
        patched_pll = (work / "sys/pll_hdmi.v").read_text()
        required_counts = {
            BRIDGE_MAPPING: 1,
            APPLY_BUNDLE: 1,
            "if(magik_response_valid) io_dout_sys <= magik_response_data;": 2,
            "mister_magik_hdmi_lock_evidence magik_hdmi_lock_evidence": 1,
            "mister_magik_video_diagnostics_control magik_video_diagnostics": 0,
            "mister_magik_video_diagnostics_avalon magik_video_diagnostics_avalon": 0,
            "mister_magik_video_diagnostics_output magik_video_diagnostics_output": 0,
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
        if not re.search(r"\.locked\s*\(\s*\)", patched_pll):
            fail("HDMI PLL wrapper no longer terminates its redundant lock output")
        if "locked.export" in patched_pll or ".locked(hdmi_pll_locked)" in patched:
            fail("diagnostics must not add a second HDMI PLL lock export")
        if patched.count("wire hdmi_pll_locked = reconfig_from_pll[16];") != 1:
            fail("sys_top does not observe the existing real HDMI PLL lock status bit")
        if patched.count("wire hdmi_pll_locked = 1'b0;") != 1:
            fail("sys_top HDMI-disabled PLL status fallback is missing")
        if patched.count("reg magik_selected_direct;") != 1 or patched.count(
            "reg hdmi_out_direct;"
        ) != 1:
            fail("final mux provenance registers are missing or ambiguous")
        if patched.count(
            "magik_selected_direct <= ~vga_fb & direct_video;"
        ) != 1 or patched.count(
            "hdmi_out_direct <= magik_selected_direct;"
        ) != 1:
            fail("final mux provenance is not aligned through two HDMI output stages")
        if len(re.findall(r"\bhdmi_out_direct\b", patched)) != 4:
            fail("final mux provenance tag has unexpected fanout")
        if re.search(
            r"\b(?:LFB_|FB_|hdmi_out_|vbuf_|reset_req)[A-Za-z0-9_]*\s*"
            r"(?:<=|=)\s*magik_diag_",
            patched,
        ):
            fail("diagnostic output reaches a functional datapath assignment")
        if re.search(
            r"magik_diag_(?:snapshot|monitor|generation|route|expected|avalon|output)",
            patched,
        ):
            fail("retired wide diagnostic wiring remains in patched sys_top")

        control_binding = re.search(
            r"mister_magik_hdmi_lock_evidence\s+"
            r"magik_hdmi_lock_evidence\s*\((.*?)\);",
            patched,
            re.S,
        )
        if control_binding is None:
            fail("missing minimal HDMI lock evidence binding")
        if control_binding.group(1).count(".hdmi_pll_locked(hdmi_pll_locked)") != 1:
            fail("lock evidence does not observe the real HDMI PLL lock")
        expected_lock_ports = sorted(
            (
                ("clk_sys", "clk_sys"),
                ("hdmi_tx_clk", "hdmi_tx_clk"),
                ("io_uio", "io_uio"),
                ("io_strobe", "io_strobe"),
                ("io_din", "io_din"),
                ("hdmi_pll_locked", "hdmi_pll_locked"),
                ("hdmi_out_vs", "hdmi_out_vs"),
                ("hdmi_out_de", "hdmi_out_de"),
                ("hdmi_out_d", "hdmi_out_d"),
                ("hdmi_out_direct", "hdmi_out_direct"),
                ("response_valid", "magik_diag_response_valid"),
                ("response_data", "magik_diag_response_data"),
            )
        )
        actual_lock_ports = sorted(
            re.findall(
                r"\.([A-Za-z_][A-Za-z0-9_]*)\s*\(\s*"
                r"([A-Za-z_][A-Za-z0-9_]*)\s*\)",
                control_binding.group(1),
            )
        )
        all_lock_port_names = re.findall(
            r"\.([A-Za-z_][A-Za-z0-9_]*)\s*\(", control_binding.group(1)
        )
        if actual_lock_ports != expected_lock_ports or len(all_lock_port_names) != 12:
            fail("HDMI lock and final-output evidence port map is not exact")
        for response_net in ("magik_diag_response_valid", "magik_diag_response_data"):
            if len(re.findall(rf"\b{response_net}\b", patched)) != 4:
                fail(f"HDMI lock response net use is not exact: {response_net}")
        if re.search(
            r"\.(?:vbuf|reset|route|snapshot|fault|heartbeat|generation)",
            control_binding.group(1),
        ):
            fail("HDMI evidence binding regained a retired observer input")
        if re.search(r"\.hdmi_pll_locked(?:_async)?\s*\(\s*led_locked\s*\)", patched):
            fail("diagnostics must not observe the adjustment-PLL LED signal")

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
                    str(diagnostics_control),
                    str(integration_tb),
                ],
                check=True,
            )
            subprocess.run(["vvp", str(simulation)], check=True)

    print(f"COVER LATCH-009 pinned Menu production bridge and opcode ownership ({commit})")
    print("FPGA latch integration check passed")


if __name__ == "__main__":
    main()
