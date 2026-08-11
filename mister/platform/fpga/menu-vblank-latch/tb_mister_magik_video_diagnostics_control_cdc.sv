// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

`timescale 1ns/1ps
`default_nettype none

module video_diagnostics_control_cdc_case #(
	parameter integer FAULT_KIND = 0
) (
	input wire clk_sys,
	output reg done = 1'b0
);
	`include "mister_magik_video_diagnostics_protocol.svh"

	reg hdmi_vbl = 1'b0, heartbeat = 1'b0, apply = 1'b0;
	reg reset_req = 1'b0, reset_out = 1'b0, cfg_done = 1'b1, pll_locked = 1'b1;
	wire snapshot_request;

	mister_magik_video_diagnostics_control dut (
		.clk_sys(clk_sys), .hdmi_vbl(hdmi_vbl), .io_uio(1'b0), .io_strobe(1'b0),
		.io_osd(1'b0), .io_din(16'd0), .apply_accepted(apply), .pending(1'b0),
		.pending_seq(16'd2), .active_seq(16'd1), .post_count(16'd1),
		.active_route_epoch(16'd1), .route_en(1'b1), .route_flt(1'b0),
		.route_fmt(6'd4), .route_width(12'd960), .route_height(12'd540),
		.route_hmin(12'd0), .route_hmax(12'd959), .route_vmin(12'd0),
		.route_vmax(12'd539), .route_base(32'h227e9000), .route_stride(14'd1920),
		.lfb_en(1'b1), .lfb_flt(1'b0), .lfb_fmt(6'd4), .lfb_width(12'd960),
		.lfb_height(12'd540), .lfb_hmin(12'd0), .lfb_hmax(12'd959),
		.lfb_vmin(12'd0), .lfb_vmax(12'd539), .lfb_base(32'h227e9000),
		.lfb_stride(14'd1920), .reset_req(reset_req), .reset_out(reset_out),
		.cfg_done(cfg_done), .pll_adjust_locked(pll_locked),
		.output_heartbeat_toggle_async(heartbeat), .avalon_fault_toggle_async(1'b0),
		.avalon_trigger_async(8'd0), .avalon_snapshot_ack_async(snapshot_request),
		.avalon_snapshot_payload_async(496'd0), .output_fault_toggle_async(1'b0),
		.output_trigger_async(8'd0), .output_snapshot_ack_async(snapshot_request),
		.output_snapshot_payload_async(496'd0),
		.snapshot_request_toggle(snapshot_request), .monitor_armed(),
		.diagnostic_generation(), .route_context_toggle(), .expected_base(),
		.expected_slot_end(), .expected_route_epoch(), .expected_active_seq(),
		.expected_route_flags(), .response_valid(), .response_data()
	);

	task automatic vblank_async;
		begin
			#2 hdmi_vbl = 1'b1; heartbeat = ~heartbeat;
			#13 hdmi_vbl = 1'b0;
			#15;
		end
	endtask

	reg [15:0] expected_flag;
	initial begin
		repeat(5) @(negedge clk_sys);
		apply = 1'b1;
		@(negedge clk_sys); apply = 1'b0;
		vblank_async();
		vblank_async();
		vblank_async();
		repeat(3) @(negedge clk_sys);
		if(!dut.monitor_armed) $fatal(1, "CDC case %0d did not arm", FAULT_KIND);

		case(FAULT_KIND)
			0: begin reset_req = 1'b1; expected_flag = 16'h0001; end
			1: begin reset_out = 1'b1; expected_flag = 16'h0002; end
			2: begin pll_locked = 1'b0; expected_flag = 16'h0008; end
			default: begin cfg_done = 1'b0; expected_flag = 16'h0004; end
		endcase
		#1;
		if(dut.trigger != MAGIK_VIDEO_DIAGNOSTICS_TRIGGER_NONE)
			$fatal(1, "CDC case %0d reacted before clk_sys sampling", FAULT_KIND);
		repeat(5) @(negedge clk_sys);
		if(dut.trigger != MAGIK_VIDEO_DIAGNOSTICS_TRIGGER_CONTROL_OR_CLOCK)
			$fatal(1, "CDC case %0d did not trigger", FAULT_KIND);
		if((dut.control_fault_flags & expected_flag) == 0)
			$fatal(1, "CDC case %0d lost fault flag %h", FAULT_KIND, expected_flag);
		done = 1'b1;
	end
endmodule

module tb_mister_magik_video_diagnostics_control_cdc;
	reg clk_sys = 1'b0;
	always #5 clk_sys = ~clk_sys;
	wire done_reset_req, done_reset_out, done_pll, done_cfg;

	video_diagnostics_control_cdc_case #(.FAULT_KIND(0)) reset_req_case
		(.clk_sys(clk_sys), .done(done_reset_req));
	video_diagnostics_control_cdc_case #(.FAULT_KIND(1)) reset_out_case
		(.clk_sys(clk_sys), .done(done_reset_out));
	video_diagnostics_control_cdc_case #(.FAULT_KIND(2)) pll_case
		(.clk_sys(clk_sys), .done(done_pll));
	video_diagnostics_control_cdc_case #(.FAULT_KIND(3)) cfg_case
		(.clk_sys(clk_sys), .done(done_cfg));

	initial begin
		wait(done_reset_req && done_reset_out && done_pll && done_cfg);
		$display("video diagnostics control CDC tests passed");
		$finish;
	end
endmodule

`default_nettype wire
