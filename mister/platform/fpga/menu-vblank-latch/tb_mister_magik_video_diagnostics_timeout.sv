// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

`timescale 1ns/1ps
`default_nettype none

module tb_mister_magik_video_diagnostics_timeout;
	reg clk = 1'b0;
	always #5 clk = ~clk;
	reg vbl = 1'b0, uio = 1'b0, strobe = 1'b0, apply = 1'b0;
	reg [15:0] din = 16'd0;
	wire request, armed, response_valid;
	wire [15:0] response_data;

	mister_magik_video_diagnostics_control #(
		.HEARTBEAT_TIMEOUT_CYCLES(24'd20),
		.SNAPSHOT_TIMEOUT_CYCLES(12'd10)
	) dut (
		.clk_sys(clk), .hdmi_vbl(vbl), .io_uio(uio), .io_strobe(strobe),
		.io_osd(1'b0), .io_din(din), .apply_accepted(apply), .pending(1'b0),
		.pending_seq(16'd2),
		.active_seq(16'd1), .post_count(16'd1), .active_route_epoch(16'd1),
		.route_en(1'b1), .route_flt(1'b0), .route_fmt(6'd4),
		.route_width(12'd960), .route_height(12'd540), .route_hmin(12'd0),
		.route_hmax(12'd959), .route_vmin(12'd0), .route_vmax(12'd539),
		.route_base(32'h227e9000), .route_stride(14'd1920),
		.lfb_en(1'b1), .lfb_flt(1'b0), .lfb_fmt(6'd4),
		.lfb_width(12'd960), .lfb_height(12'd540), .lfb_hmin(12'd0),
		.lfb_hmax(12'd959), .lfb_vmin(12'd0), .lfb_vmax(12'd539),
		.lfb_base(32'h227e9000), .lfb_stride(14'd1920),
		.reset_req(1'b0), .reset_out(1'b0), .cfg_done(1'b1),
		.hdmi_pll_locked(1'b1), .output_heartbeat_toggle_async(1'b0),
		.avalon_fault_toggle_async(1'b0), .avalon_trigger_async(8'd0),
		.avalon_snapshot_ack_async(1'b0), .avalon_snapshot_payload_async(240'd0),
		.output_fault_toggle_async(1'b0), .output_trigger_async(8'd0),
		.output_snapshot_ack_async(1'b0), .output_snapshot_payload_async(240'd0),
		.snapshot_request_toggle(request), .monitor_armed(armed),
		.hdmi_pll_locked_sync(),
		.diagnostic_generation(), .route_context_toggle(), .expected_base(),
		.expected_route_epoch(), .expected_active_seq(),
		.expected_route_flags(), .response_valid(response_valid),
		.response_data(response_data)
	);

	task automatic vblank;
		begin
			@(negedge clk); vbl = 1'b1;
			@(negedge clk); vbl = 1'b0;
		end
	endtask

	task automatic send_word;
		input [15:0] value;
		begin
			@(negedge clk); din = value; strobe = 1'b1;
			@(negedge clk); strobe = 1'b0;
		end
	endtask

	reg [15:0] state_word;
	initial begin
		repeat(3) @(negedge clk);
		apply = 1'b1;
		@(negedge clk); apply = 1'b0;
		vblank(); vblank(); vblank();
		if(!armed) $fatal(1, "timeout monitor did not arm");
		repeat(36) @(negedge clk);
		if(!request) $fatal(1, "stopped HDMI heartbeat did not request a freeze");

		uio = 1'b1;
		@(negedge clk); din = 16'h005d; strobe = 1'b1;
		#1 if(!response_valid || response_data != 16'h4d4d) $fatal(1, "control magic missing");
		@(negedge clk); strobe = 1'b0;
		send_word(16'd0);
		@(negedge clk); din = 16'd0; strobe = 1'b1;
		#1 state_word = response_data;
		@(negedge clk); strobe = 1'b0; uio = 1'b0;
		if((state_word & 16'h0023) != 16'h0023)
			$fatal(1, "stopped-domain snapshot was not partial: %h", state_word);
		$display("video diagnostics stopped-clock timeout tests passed");
		$finish;
	end
endmodule

`default_nettype wire
