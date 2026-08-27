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
    "0x66": re.compile(r"(?:cmd|io_din\s*\[\s*7\s*:\s*0\s*\])\s*==\s*(?:8\s*'h|')66", re.I),
    "0x67": re.compile(r"(?:cmd|io_din\s*\[\s*7\s*:\s*0\s*\])\s*==\s*(?:8\s*'h|')67", re.I),
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
    local_signoff_profile = source_dir / "local-signoff-profile.txt"
    timing_report = source_dir / "report_top_timing.tcl"
    integration_tb = source_dir / "tb_mister_magik_sys_top_integration.sv"
    raw_scaler_diagnostic_tb = (
        source_dir / "tb_mister_magik_video_diagnostics_control.sv"
    )
    completion_queue_tb = source_dir / "tb_mister_magik_scaler_completion_queue.vhd"
    completion_formal_dut = (
        source_dir / "mister_magik_scaler_completion_formal_dut.vhd"
    )
    completion_formal_wrapper = (
        source_dir / "mister_magik_ascal_completion_formal.sv"
    )
    copy_tail_formal_dut = (
        source_dir / "mister_magik_scaler_copy_tail_formal_dut.vhd"
    )
    copy_tail_formal_wrapper = (
        source_dir / "mister_magik_scaler_copy_tail_formal.sv"
    )
    completion_formal_check = (
        root / "scripts/checks/check-fpga-scaler-completion-formal.py"
    )
    for formal_input in (
        completion_formal_dut,
        completion_formal_wrapper,
        copy_tail_formal_dut,
        copy_tail_formal_wrapper,
        completion_formal_check,
        raw_scaler_diagnostic_tb,
    ):
        if not formal_input.is_file():
            fail(f"scaler completion formal input is missing: {formal_input}")
    formal_dut_source = completion_formal_dut.read_text()
    formal_wrapper_source = completion_formal_wrapper.read_text()
    copy_tail_dut_source = copy_tail_formal_dut.read_text()
    copy_tail_wrapper_source = copy_tail_formal_wrapper.read_text()
    formal_check_source = completion_formal_check.read_text()
    for fragment in (
        "USE work.mister_magik_scaler_completion_queue.ALL;",
        "return_credits_next(",
        "return_phase_next(",
        "return_words_remaining(",
        "return_accounting_invalid(",
        "return_drain_ready(",
        "completion_queue_next(",
        "completion_queue_overflow(",
        "align_event<='1' WHEN",
        "release_event<=align_event AND return_drain;",
        "IF align_event='1' THEN",
        "read_obligation_accept(",
        "avl_reset_n='0' OR read_reset_seen='0')",
        "ELSIF issue_event='1' THEN",
    ):
        if fragment not in formal_dut_source:
            fail(f"formal DUT is detached from production transition: {fragment}")
    for fragment in (
        "cover_two_stopped_delivered",
        "cover_coincident_ack_completion",
        "cover_final_old_beat_during_reset",
        "cover_old_beat_after_reset",
        "cover_vs_alignment_during_drain",
        "cover_first_post_drain_completion",
        "cover_active_credit_vs",
        "cover_issue_empty_vs",
        "cover_final_return_vs_wait",
        "reference_words",
        "(* anyseq *) wire waitrequest;",
        "if (align_event) begin",
        "vs_edge && words_remaining != 0",
    ):
        if fragment not in formal_wrapper_source:
            fail(f"formal environment obligation is missing: {fragment}")
    for fragment in (
        "USE work.mister_magik_scaler_completion_queue.ALL;",
        "copy_shift_active(false,0,'1')",
        "copy_shift_onext((acpt+1) MOD 16,active_format,128)",
        "active_format<=format;",
        "copy_terminal_ready(",
        "pixel_valid_o<='0';",
    ):
        if fragment not in copy_tail_dut_source:
            fail(f"copy-tail formal DUT is detached from production helper: {fragment}")
    for fragment in (
        "assert(shift);",
        "assert(!pixel_valid);",
        "assert(age < 18);",
        "assert(terminal);",
        "retired <= 1'b1;",
        "cover(retired);",
    ):
        if fragment not in copy_tail_wrapper_source:
            fail(f"copy-tail formal obligation is missing: {fragment}")
    for fragment in (
        'run(["git", "apply", "--recount", str(patch)]',
        '"ghdl",\n                "synth",',
        "-tempinduct",
        '"patched_ascal_sha256"',
        '"ghdl_netlist_sha256"',
        '"copy_tail_bounded_depth": 48',
    ):
        if fragment not in formal_check_source:
            fail(f"formal checker source binding is missing: {fragment}")
    rtl_source = rtl.read_text()
    bridge_source = bridge.read_text()
    control_source = diagnostics_control.read_text()
    avalon_source = diagnostics_avalon.read_text()
    output_source = diagnostics_output.read_text()
    if local_signoff_profile.read_text().strip() != "experimental_scaler_fetch-v1":
        fail("scaler-fetch candidate lacks its exact local signoff profile identity")
    if control_source.count("module mister_magik_scaler_fetch_ordered_frame (") != 1:
        fail("scaler-fetch ordered-frame observer design unit is missing or ambiguous")
    if re.search(r"(?m)^\s*module\b", avalon_source + output_source):
        fail("retired Avalon/output compatibility sources must define no design unit")
    for required_observer_fragment in (
        "input  wire         clk_100m",
        "input  wire         clk_sys",
        "input  wire [27:0]  vbuf_address",
        "input  wire [7:0]   vbuf_burstcount",
        "input  wire         vbuf_waitrequest",
        "input  wire [127:0] vbuf_readdata",
        "input  wire         vbuf_readdatavalid",
        "input  wire         vbuf_read",
        "localparam [15:0] FETCH_STATE_SCHEMA = MAGIK_RAW_SCALER_STATE_SCHEMA;",
        "MAGIK_RAW_SCALER_STATE_FLAG_CAPTURE_VALID[6:0];",
        "MAGIK_RAW_SCALER_STATE_FLAG_FIFO_OVERFLOW[6:0];",
        "MAGIK_RAW_SCALER_STATE_FLAG_UNEXPECTED_RETURN[6:0];",
        "MAGIK_RAW_SCALER_STATE_FLAG_BAD_BURSTCOUNT[6:0];",
        "MAGIK_RAW_SCALER_STATE_FLAG_BAD_RETURN_PHASE[6:0];",
        "MAGIK_RAW_SCALER_STATE_FLAG_EPOCH_OVERLAP[6:0];",
        "MAGIK_RAW_SCALER_STATE_FLAG_COUNTER_OVERFLOW[6:0];",
        "MAGIK_RAW_SCALER_STATE_CAPTURE_SEQUENCE_WORD",
        "wire accepted = vbuf_read && !vbuf_waitrequest;",
        "wire return_has_entry = returned && fifo_count != 2'd0;",
        "wire return_last = return_has_entry && return_phase == 7'd127;",
        "vbuf_burstcount == REQUIRED_BURSTCOUNT",
        "vbuf_address[27:7] < previous_address",
        "reg [7:0]  fifo_address_token0",
        "reg [7:0]  fifo_address_token1",
        "reg        fifo_wrap0",
        "reg        fifo_wrap1",
        "reg [1:0]  fifo_count",
        "reg [6:0]  return_phase",
        "function automatic [15:0] fold_return_data;",
        "function automatic [7:0] fold_address;",
        "fold_return_data(vbuf_readdata)",
        "function automatic [15:0] ordered_signature_update;",
        "mixed = signature_in ^ token_in;",
        "{mixed[14:0], mixed[15] ^ mixed[0]};",
        "case({enqueue, dequeue})",
        "return_token = fold_return_data(vbuf_readdata) ^ TOKEN_DATA;",
        "SIGNATURE_INITIAL,",
        "{TOKEN_ADDRESS, fifo_address_token0}",
        "epoch_signature,",
        "if(return_phase == 7'd0 && fifo_wrap0) begin",
        "published_signature <= epoch_signature;",
        "published_flags <= FETCH_FLAG_CAPTURE_VALID;",
        "fault_event[6:1] != 6'd0",
        "published_flags <= fault_event;",
        "(* preserve, dont_replicate *) reg source_generation = 1'b0;",
        "source_generation <= ~source_generation;",
        "generation_meta <= source_generation;",
        "generation_sync <= generation_meta;",
        "snapshot_signature <= published_signature;",
        "snapshot_flags <= published_flags;",
        "snapshot_sequence <= snapshot_sequence + 1'd1;",
        "function automatic [15:0] crc16_update_bit;",
        "snapshot_crc <= FETCH_SCHEMA_CRC;",
        "crc_bit_count <= 6'd48;",
        "snapshot_crc <= next_crc;",
    ):
        if required_observer_fragment not in control_source:
            fail(
                "scaler-fetch ordered-frame observer structure is missing: "
                f"{required_observer_fragment}"
            )
    if re.search(r"\breg\s*\[127:0\]", control_source):
        fail("scaler-fetch observer must not retain a 128-bit return-data isolation register")
    if "generation_launch" in control_source:
        fail("rejected placement-heavy generation launch stage remains present")
    for forbidden_observer_input in (
        "clk_hdmi",
        "raw_ce",
        "raw_rgb",
        "raw_de",
        "raw_hs",
        "raw_vs",
        "LFB_",
        "hdmi_data_mask",
        "hdmi_data_osd",
        "hdmi_out_",
        "shadowmask",
        "pll",
        "framebuffer",
        "completion_pending",
        "o_copylev",
        "o_readlev",
        "reg [127:0]",
        "o_copy",
        "o_readdata",
        "avl_magik",
    ):
        if forbidden_observer_input in control_source:
            fail(f"scaler-fetch observer exceeds its passive tap boundary: {forbidden_observer_input}")
    for retired_control_observer in (
        "mister_magik_raw_scaler_ordered_frame",
        "mister_magik_raw_scaler_diagnostic",
        "isolated_rgb",
        "signature_rgb",
        "pixel_token",
        "line_end_token",
        "control_crc",
        "candidate_streak",
        "candidate_valid",
        "baseline_valid",
        "baseline_matches",
        "mismatch_latched",
        "first_active_rgb",
        "variation_seen",
        "crc32c_update_byte",
        "crc32c_update_pixel",
        "published_oldest_crc",
        "published_previous_crc",
        "published_newest_crc",
        "variation_history",
        "comparison_count",
        "frame_pixels",
        "frame_lines",
        "(* preserve *) reg [63:0] snapshot_state",
        "wire [63:0] source_state",
        "reg [63:0] snapshot_state",
        "wire [47:0] source_state",
        "reg [47:0] snapshot_state",
        "wire [31:0] source_state",
        "published_sequence",
        "pipeline_state",
        "pipeline_generation",
        "isolated_rgb",
        "signature_rgb",
        "mister_magik_raw_scaler_ordered_frame",
    ):
        if retired_control_observer in control_source + avalon_source + output_source:
            fail(f"retired disposable observer remains: {retired_control_observer}")
    compiled_diagnostics = avalon_source + output_source
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
            fail(f"retired wide diagnostic fragment remains: {retired_fragment}")
    for retired_reader_fragment in (
        "command_kind",
        "snapshot_path_extra",
        "scaler_fetch_state_meta",
        "scaler_fetch_state_sys",
        "output_no_de_toggle",
        "raw_no_de_toggle",
        "post_no_de_toggle",
        "avalon_bucket_toggle",
    ):
        if retired_reader_fragment in control_source:
            fail(f"retired broad observer remains: {retired_reader_fragment}")
    for fragment in (
        '`include "mister_magik_video_diagnostics_protocol.svh"',
        "else if(cmd_start && evidence_command) begin",
        "tx_crc <= evidence_header_crc;",
        "else if(cmd_data && evidence_command) begin",
        "else if(evidence_command && (word_index < evidence_crc_word)) begin",
        "tx_crc <= crc_word(tx_crc, evidence_word);",
    ):
        if fragment in rtl_source:
            fail(f"diagnostic serializer remains in the latch: {fragment}")
    for fragment in (
        ".evidence_word(evidence_word)",
        "assign evidence_command = command_id;",
        "assign evidence_snapshot = command_start;",
    ):
        if fragment in bridge_source:
            fail(f"diagnostic selection remains in the latch bridge: {fragment}")
    for redundant_register in (
        "output_no_de_previous",
        "output_black_direct_previous",
        "output_black_scaled_previous",
        "output_black_mixed_previous",
        "output_de_has_nonzero_previous",
        "snapshot_output_nonzero_count",
    ):
        if redundant_register in control_source:
            fail(f"retired diagnostic state remains: {redundant_register}")
    diagnostics_sdc_text = diagnostics_sdc.read_text()
    timing_report_text = timing_report.read_text()
    unconstrained_report = (
        "report_ucp \\\n"
        "\t-file output_files/menu.unconstrained-paths.rpt"
    )
    if timing_report_text.count(unconstrained_report) != 1:
        fail("full unconstrained-path timing report is missing or ambiguous")
    diagnostic_net_delay_report = (
        "report_net_delay \\\n"
        '\t-panel_name "MagiK Diagnostic CDC Net Delay" \\\n'
        "\t-nworst 100 \\\n"
        "\t-file output_files/menu.magik-diagnostic-cdc-net-delay.rpt"
    )
    if timing_report_text.count(diagnostic_net_delay_report) != 1:
        fail("completion net-delay report is missing or ambiguous")
    if "-nworst 50" in timing_report_text:
        fail("diagnostic net-delay report retains the truncated schema-4 depth")
    timing_commands = re.findall(
        r"(?m)^\s*(set_[A-Za-z0-9_]+\b[^\n]*)$", diagnostics_sdc_text
    )
    if timing_commands != ["set_net_delay -max 10.0 \\"] * 3:
        fail("diagnostic SDC must contain the two completion bounds and one observer bound")
    for fragment in (
        "{*ascal:ascal|avl_readdataack} 1",
        "{*ascal:ascal|o_readdataack_sync} 1",
        "{*ascal:ascal|o_readdataack_sync2} 1",
        "{*ascal:ascal|avl_completion_ack_meta} 1",
        "-from $magik_scaler_completion_request",
        "-to $magik_scaler_completion_request_meta",
        "-from $magik_scaler_completion_ack_route",
        "-to $magik_scaler_completion_ack_meta",
        "{*mister_magik_scaler_fetch_ordered_frame:magik_scaler_fetch_ordered_frame|source_generation} 1",
        "{*mister_magik_scaler_fetch_ordered_frame:magik_scaler_fetch_ordered_frame|generation_meta} 1",
        "-from $magik_fetch_frame_generation",
        "-to $magik_fetch_frame_generation_meta",
        "MagiK diagnostics CDC analysis applied: scaler_completion_request_ack scaler_copy_tail scaler_fetch_ordered_signature",
        "*ascal:ascal|o_readdataack_sync2*",
        "scaler_copy_tail",
    ):
        if diagnostics_sdc_text.count(fragment) != 1:
            fail(f"scaler completion request/ack constraint is missing or ambiguous: {fragment}")
    for forbidden_sdc in (
        "set_false_path",
        "magik_require_data_pin",
        "control_pll_lock",
        "magik_raw_scaler_diagnostic",
    ):
        if forbidden_sdc in diagnostics_sdc_text:
            fail(f"retired HDMI lock constraint remains: {forbidden_sdc}")
    with tempfile.TemporaryDirectory(prefix="mister-magik-fpga-integration-") as temporary:
        work = Path(temporary) / "Menu_MiSTer"
        shutil.copytree(menu, work, ignore=shutil.ignore_patterns(".git", "db", "output_files"))
        subprocess.run(
            ["git", "apply", "--recount", "--check", str(patch)],
            cwd=work,
            check=True,
        )
        subprocess.run(
            ["git", "apply", "--recount", str(patch)], cwd=work, check=True
        )
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
            "set_global_assignment -name SDC_FILE "
                "sys/mister_magik_video_diagnostics.sdc\n"
            )

        patched = (work / "sys/sys_top.v").read_text()
        patched_ascal = (work / "sys/ascal.vhd").read_text()
        patched_sysmem = (work / "sys/sysmem.sv").read_text()
        patched_terminator = (work / "sys/f2sdram_safe_terminator.sv").read_text()
        patched_pll = (work / "sys/pll_hdmi.v").read_text()
        required_counts = {
            BRIDGE_MAPPING: 1,
            APPLY_BUNDLE: 1,
            "if(magik_response_valid) io_dout_sys <= magik_response_data;": 2,
            "if(magik_diag_response_valid) io_dout_sys <= magik_diag_response_data;": 2,
            "mister_magik_hdmi_lock_evidence magik_hdmi_lock_evidence": 0,
            "mister_magik_scaler_completion_cdc magik_scaler_completion_cdc": 0,
            "mister_magik_video_diagnostics_control magik_video_diagnostics": 0,
            "mister_magik_video_diagnostics_avalon magik_video_diagnostics_avalon": 0,
            "mister_magik_video_diagnostics_output magik_video_diagnostics_output": 0,
            "mister_magik_scaler_fetch_ordered_frame magik_scaler_fetch_ordered_frame": 1,
            "magik_diag_response_valid": 4,
            "magik_diag_response_data": 4,
        }
        mismatches = [
            f"{fragment.splitlines()[0]!r} expected {expected}, found {patched.count(fragment)}"
            for fragment, expected in required_counts.items()
            if patched.count(fragment) != expected
        ]
        if mismatches:
            fail("patched production bridge binding mismatch: " + "; ".join(mismatches))
        required_observer_bindings = {
            ".reset_active(reset_req)": 1,
            # One production ascal binding plus one passive observer binding.
            ".vbuf_address(vbuf_address)": 2,
            ".vbuf_burstcount(vbuf_burstcount)": 2,
            ".vbuf_waitrequest(vbuf_waitrequest)": 2,
            ".vbuf_readdata(vbuf_readdata)": 2,
            ".vbuf_readdatavalid(vbuf_readdatavalid)": 2,
            ".vbuf_read(vbuf_read)": 2,
        }
        for binding, expected_count in required_observer_bindings.items():
            if patched.count(binding) != expected_count:
                fail(
                    "scaler-fetch production tap mismatch: "
                    f"{binding} expected {expected_count}, found {patched.count(binding)}"
                )
        observer_mapping = re.compile(
            r"mister_magik_scaler_fetch_ordered_frame\s+"
            r"magik_scaler_fetch_ordered_frame\s*\(.*?"
            r"\.clk_100m\(clk_100m\).*?\.clk_sys\(clk_sys\).*?"
            r"\.reset_active\(reset_req\).*?\.vbuf_address\(vbuf_address\).*?"
            r"\.vbuf_burstcount\(vbuf_burstcount\).*?"
            r"\.vbuf_waitrequest\(vbuf_waitrequest\).*?"
            r"\.vbuf_readdata\(vbuf_readdata\).*?"
            r"\.vbuf_readdatavalid\(vbuf_readdatavalid\).*?"
            r"\.vbuf_read\(vbuf_read\).*?"
            r"\.response_valid\(magik_diag_response_valid\).*?"
            r"\.response_data\(magik_diag_response_data\).*?\);",
            re.S,
        )
        if len(observer_mapping.findall(patched)) != 1:
            fail("scaler-fetch ordered-frame observer mapping is missing or ambiguous")
        for retired_binding in (
            "magik_scaler_copy_state",
            ".magik_diag_state",
            ".magik_diag_generation",
            "mister_magik_raw_scaler_ordered_frame",
            "mister_magik_raw_scaler_diagnostic",
        ):
            if retired_binding in patched:
                fail(f"retired disposable observer binding remains: {retired_binding}")
        for fragment in (
            "magik_scaler_completion_gray",
            "magik_scaler_completion_pulse",
            ".avl_completion_gray(",
            ".o_completion_pulse(",
        ):
            if fragment in patched:
                fail(f"external scaler completion round trip remains: {fragment}")
        required_completion_counts = {
            "PACKAGE mister_magik_scaler_completion_queue IS": 1,
            "PACKAGE BODY mister_magik_scaler_completion_queue IS": 1,
            "USE work.mister_magik_scaler_completion_queue.ALL;": 1,
            "FUNCTION completion_queue_next(": 2,
            "FUNCTION completion_queue_overflow(": 2,
            "FUNCTION return_credits_next(": 2,
            "FUNCTION return_phase_next(": 2,
            "FUNCTION return_words_remaining(": 2,
            "FUNCTION return_accounting_invalid(": 2,
            "FUNCTION read_obligation_accept(": 2,
            "FUNCTION return_drain_ready(": 2,
            "FUNCTION copy_shift_active(": 2,
            "FUNCTION copy_shift_onext(": 2,
            "FUNCTION copy_terminal_ready(": 2,
            "state_v:=request_toggle & completion_pending;": 1,
            "state_v(0):=completion;": 1,
            "RETURN request_toggle/=completion_ack AND": 1,
            "SIGNAL avl_readdataack,avl_completion_pending : std_logic;": 1,
            "SIGNAL avl_completion_ack_meta,avl_completion_ack_sync : std_logic;": 1,
            "SIGNAL avl_return_drain : std_logic:='1';": 1,
            "SIGNAL avl_return_credits : natural RANGE 0 TO 2:=0;": 1,
            "SIGNAL avl_return_phase : natural RANGE 0 TO BLEN-1:=0;": 1,
            "SIGNAL avl_read_accepted : std_logic:='0';": 1,
            "ATTRIBUTE preserve OF avl_readdataack : SIGNAL IS true;": 1,
            "SIGNAL o_readdataack,o_readdataack_sync,o_readdataack_sync2 : std_logic;": 1,
            "SYNCHRONIZER_IDENTIFICATION FORCED\";": 1,
            "SYNCHRONIZER_IDENTIFICATION FORCED_IF_ASYNCHRONOUS\";": 1,
            "SYNCHRONIZER_IDENTIFICATION FORCED; -name PRESERVE_REGISTER ON\";": 1,
            "SYNCHRONIZER_IDENTIFICATION FORCED_IF_ASYNCHRONOUS; -name PRESERVE_REGISTER ON\";": 1,
            "avl_completion_ack_meta<=o_readdataack_sync2; -- <ASYNC>": 1,
            "avl_completion_ack_sync<=avl_completion_ack_meta;": 1,
            "AvalonReturnAccounting:PROCESS(avl_clk) IS": 1,
            "issued_v:=read_obligation_accept(": 1,
            "avl_read_i,avl_read_accepted,avl_waitrequest,": 1,
            "avl_reset_na='0' OR avl_state=sREAD);": 1,
            "returned_v:=avl_readdatavalid='1';": 1,
            "ASSERT NOT return_accounting_invalid(": 1,
            "avl_return_credits<=return_credits_next(": 1,
            "avl_return_phase<=return_phase_next(": 1,
            "IF avl_read_i='0' THEN": 1,
            "avl_read_accepted<='0';": 1,
            "ELSIF issued_v THEN": 1,
            "avl_read_accepted<='1';": 1,
            "avl_return_drain<='1';": 1,
            "IF return_drain_ready(": 1,
            "avl_return_credits,avl_return_phase) THEN": 1,
            "IF avl_return_drain='0' THEN": 1,
            "IF avl_read_i='1' AND avl_read_accepted='0' AND": 1,
            "avl_read<=avl_read_i AND NOT avl_read_accepted": 1,
            "WHEN avl_reset_na='0' OR avl_state=sREAD ELSE '0';": 1,
            "IF avl_readdatavalid='1' AND avl_return_drain='0' THEN": 1,
            "avl_wad<=(avl_wad+1) MOD (2*BLEN);": 1,
            "IF (avl_wad MOD BLEN)=BLEN-2 THEN": 1,
            "completion_v:='1';": 1,
            "completion_state_v:=completion_queue_next(": 1,
            "avl_readdataack<=completion_state_v(1);": 1,
            "avl_completion_pending<=completion_state_v(0);": 1,
            "ASSERT NOT completion_queue_overflow(": 1,
            'REPORT "scaler completion queue overflow" SEVERITY failure;': 1,
            "o_readdataack_sync<=avl_readdataack; -- <ASYNC>": 1,
            "o_readdataack_sync2<=o_readdataack_sync;": 1,
            "o_readdataack<=o_readdataack_sync XOR o_readdataack_sync2;": 1,
            "IF lev_dec_v='1' AND o_readdataack='0' THEN": 1,
            "ELSIF lev_dec_v='0' AND o_readdataack='1' THEN": 1,
            "RETURN copy_shift_onext(acpt,format(2 DOWNTO 0),N_DW);": 1,
            "IF copy_shift_active(hcarry_v,o_dshi,o_last) THEN": 1,
            "IF NOT hcarry_v AND o_dshi=0 AND o_last='1' THEN": 1,
            "o_copyv(0)<='0';": 2,
            "IF copy_terminal_ready(": 1,
        }
        for fragment, expected_count in required_completion_counts.items():
            if patched_ascal.count(fragment) != expected_count:
                fail(
                    "patched ascal lossless completion logic count mismatch: "
                    f"{fragment} expected {expected_count}, "
                    f"found {patched_ascal.count(fragment)}"
                )
        for forbidden_repair in (
            "avl_completion_bin",
            "completion_gray",
            "o_completion_seen",
            "o_completion_pulse",
            "avl_outstanding_returns",
            "avl_return_aligned",
            "outstanding_returns_next",
            "outstanding_returns_invalid",
            "return_write_phase_next",
            "return_block_complete",
            "read_obligation_issue",
            "IF o_copylev>0 THEN",
            "IF o_copylev<2 THEN",
        ):
            if forbidden_repair in patched_ascal:
                fail(f"superseded completion repair state remains: {forbidden_repair}")
        for retired_observer_fragment in (
            "scheduler_diagnostic_candidate",
            "scheduler_diagnostic_word",
            "raw_rgb_staged",
            "raw_de_staged",
            "first_active_rgb",
            "variation_seen",
            "avl_magik_",
            "MagiKScalerPipelineDiagnostic",
            "o_magik_generation_meta",
            "o_magik_completed_flags",
            "o_magik_",
            "magik_diag_",
            "magik_events",
            "magik_copy_flags_next",
        ):
            if retired_observer_fragment in patched_ascal:
                fail(
                    "retired scheduler observer remains in production ascal: "
                    f"{retired_observer_fragment}"
                )
        scalaire_process = re.search(
            r"Scalaire:PROCESS\s*\(o_clk,o_reset_na\) IS(?P<body>.*?)END PROCESS Scalaire;",
            patched_ascal,
            re.S,
        )
        if scalaire_process is None:
            fail("production Scalaire process is missing")
        scalaire_body = scalaire_process.group("body")
        for exact_tail in (
            "IF copy_shift_active(hcarry_v,o_dshi,o_last) THEN",
            "IF NOT hcarry_v AND o_dshi=0 AND o_last='1' THEN\n"
            "\t\t\t\t\t\t\to_copyv(0)<='0';",
            "o_last1<=o_last;\n\t\t\t\t\t\to_last2<=o_last1;",
            "IF copy_terminal_ready(\n"
            "\t\t\t\t\t\t\to_adturn,shift_onext((o_acpt+1) MOD 16,o_format),\n"
            "\t\t\t\t\t\t\to_ad MOD BLEN=0,o_lastv(0),o_last2) THEN",
            "o_copy<=sWAIT;\n\t\t\t\t\t\t\tlev_dec_v:='1';",
        ):
            if scalaire_body.count(exact_tail) != 1:
                fail(f"copy-tail exact production binding is missing: {exact_tail}")
        for retired_tail in (
            "IF hcarry_v OR o_dshi>0 THEN",
            "IF o_adturn='1' AND shift_onext((o_acpt+1) MOD 16,o_format) AND",
        ):
            if retired_tail in scalaire_body:
                fail(f"superseded copy terminal expression remains: {retired_tail}")
        tail_suppression = re.search(
            r"IF NOT hcarry_v AND o_dshi=0 AND o_last='1' THEN\s*"
            r"o_copyv\(0\)<='0';\s*END IF;",
            scalaire_body,
        )
        if tail_suppression is None:
            fail("copy tail does not suppress new pixel validity")
        for reset_fragment in (
            "avl_readdataack<='0';",
            "avl_completion_pending<='0';",
            "avl_completion_ack_meta<='0';",
            "avl_completion_ack_sync<='0';",
            "o_readdataack<='0';",
            "o_readdataack_sync<='0';",
            "o_readdataack_sync2<='0';",
        ):
            if patched_ascal.count(reset_fragment) != 1:
                fail(f"completion transport reset is missing or ambiguous: {reset_fragment}")
        if patched_ascal.count("avl_wad<=2*BLEN-1;") != 1:
            fail("Avalon write phase must align exactly once at vertical sync")
        avalon_reset = re.search(
            r"IF avl_reset_na='0' THEN(?P<body>.*?)ELSIF rising_edge\(avl_clk\) THEN",
            patched_ascal,
            re.S,
        )
        if avalon_reset is None:
            fail("Avalon reset branch is missing")
        if "avl_wad<=2*BLEN-1;" in avalon_reset.group("body"):
            fail("Avalon write phase must not use a nonzero asynchronous reset preset")
        for retained_accounting in (
            "avl_return_credits",
            "avl_return_phase",
            "avl_read_accepted",
        ):
            if retained_accounting in avalon_reset.group("body"):
                fail(
                    "Avalon reset branch must retain return accounting: "
                    f"{retained_accounting}"
                )
        output_copy_reset = re.search(
            r"Scalaire:PROCESS\s*\(o_clk,o_reset_na\) IS.*?IF o_reset_na='0' THEN"
            r"(?P<body>.*?)ELSIF rising_edge\(o_clk\) THEN",
            patched_ascal, re.S)
        if output_copy_reset is None:
            fail("HDMI-domain scaler reset branch is missing")
        for last_reset in (
            "o_last<='0';",
            "o_last1<='0';",
            "o_last2<='0';",
        ):
            if output_copy_reset.group("body").count(last_reset) != 1:
                fail(
                    "HDMI-domain copy-tail reset is missing or ambiguous: "
                    f"{last_reset}"
                )
            if scalaire_body.count(last_reset) != 2:
                fail(f"copy-tail phase must also clear at line start: {last_reset}")
        vs_release = re.search(
            r"IF avl_o_vs_sync='0' AND avl_o_vs='1' THEN\s*"
            r"IF return_drain_ready\(\s*"
            r"avl_return_credits,avl_return_phase\) THEN\s*"
            r"avl_wad<=2\*BLEN-1;\s*"
            r"avl_return_drain<='0';\s*END IF;\s*END IF;",
            patched_ascal,
        )
        if vs_release is None:
            fail("VS phase alignment and drain release are not guarded by empty accounting")
        for topology_fragment, topology_source in (
            (".reset_core_req(reset_req)", patched),
            (".reset_na   (~reset_req)", patched),
            (".avl_readdatavalid(vbuf_readdatavalid)", patched),
            ("assign reset_out = ~init_reset_n | ~hps_h2f_reset_n | reset_core_req;", patched_sysmem),
            ("vbuf_reset_0 <= reset_out;", patched_sysmem),
            ("vbuf_reset_1 <= vbuf_reset_0;", patched_sysmem),
            (".readdatavalid_slave      (vbuf_readdatavalid)", patched_sysmem),
            ("else if (read_slave && waitrequest_master) begin", patched_terminator),
            ("read_terminating           <= 1;", patched_terminator),
            ("read_master       = read_terminating;", patched_terminator),
            ("assign readdatavalid_slave = readdatavalid_master;", patched_terminator),
        ):
            if topology_source.count(topology_fragment) != 1:
                fail(
                    "exact reset/return topology changed or is ambiguous: "
                    f"{topology_fragment}"
                )
        if not re.search(r"\.locked\s*\(\s*\)", patched_pll):
            fail("HDMI PLL wrapper no longer terminates its redundant lock output")
        if "locked.export" in patched_pll or ".locked(hdmi_pll_locked)" in patched:
            fail("diagnostics must not add a second HDMI PLL lock export")
        if "wire hdmi_pll_locked" in patched:
            fail("repair-only sys_top retains the retired HDMI PLL diagnostic tap")
        if "magik_selected_direct" in patched or "hdmi_out_direct" in patched:
            fail("retired final-mux diagnostic provenance remains")
        if re.search(
            r"\b(?:LFB_|FB_|hdmi_out_|vbuf_|reset_req)[A-Za-z0-9_]*\s*"
            r"(?:<=|=)\s*magik_diag_",
            patched,
        ):
            fail("diagnostic output reaches a functional datapath assignment")
        if re.search(r"magik_diag_(?:snapshot|monitor|route|expected|avalon|output)", patched):
            fail("retired wide diagnostic wiring remains in patched sys_top")

        for evidence_net in (
            "magik_evidence_word",
            "magik_evidence_command",
            "magik_evidence_snapshot",
            "magik_evidence_word_index",
        ):
            if re.search(rf"\b{evidence_net}\b", patched):
                fail(f"retired shared evidence net remains: {evidence_net}")
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
            "SDC_FILE sys/mister_magik_video_diagnostics.sdc",
        )
        bad_assignments = [
            assignment for assignment in assignments if qsf_text.count(assignment) != 1
        ]
        if bad_assignments:
            fail("generated QSF assignment mismatch: " + ", ".join(bad_assignments))
        for retired_assignment in (
            "SYSTEMVERILOG_FILE sys/mister_magik_video_diagnostics_avalon.sv",
            "SYSTEMVERILOG_FILE sys/mister_magik_video_diagnostics_output.sv",
        ):
            if retired_assignment in qsf_text:
                fail("retired empty source remains compiled: " + retired_assignment)

        if args.simulate:
            ghdl_work = Path(temporary) / "ghdl-work"
            ghdl_work.mkdir()
            subprocess.run(
                [
                    "ghdl",
                    "-a",
                    "--std=08",
                    f"--workdir={ghdl_work}",
                    str(work / "sys/ascal.vhd"),
                    str(completion_queue_tb),
                ],
                cwd=ghdl_work,
                check=True,
            )
            subprocess.run(
                [
                    "ghdl",
                    "-e",
                    "--std=08",
                    f"--workdir={ghdl_work}",
                    "tb_mister_magik_scaler_completion_queue",
                ],
                cwd=ghdl_work,
                check=True,
            )
            subprocess.run(
                [
                    "ghdl",
                    "-r",
                    "--std=08",
                    f"--workdir={ghdl_work}",
                    "tb_mister_magik_scaler_completion_queue",
                    "--assert-level=error",
                ],
                cwd=ghdl_work,
                check=True,
            )
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
                    str(diagnostics_avalon),
                    str(integration_tb),
                ],
                check=True,
            )
            subprocess.run(["vvp", str(simulation)], check=True)
            raw_scaler_simulation = Path(temporary) / "raw-scaler-diagnostic.vvp"
            subprocess.run(
                [
                    "iverilog",
                    "-g2012",
                    "-I",
                    str(source_dir),
                    "-s",
                    "tb_mister_magik_video_diagnostics_control",
                    "-o",
                    str(raw_scaler_simulation),
                    str(diagnostics_control),
                    str(raw_scaler_diagnostic_tb),
                ],
                check=True,
            )
            subprocess.run(["vvp", str(raw_scaler_simulation)], check=True)

    print(f"COVER LATCH-009 pinned Menu production bridge and opcode ownership ({commit})")
    print("FPGA latch integration check passed")


if __name__ == "__main__":
    main()
