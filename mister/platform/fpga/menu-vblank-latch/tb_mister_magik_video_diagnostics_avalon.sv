// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

`timescale 1ns/1ps
`default_nettype none

module tb_mister_magik_video_diagnostics_avalon;
`include "mister_magik_video_diagnostics_protocol.svh"
	reg clk_100m = 1'b0;
	always #5 clk_100m = ~clk_100m;

	reg armed = 1'b0;
	reg armed_address = 1'b0;
	reg snapshot_request = 1'b0;
	reg [15:0] generation = 16'd7;
	reg route_context = 1'b0;
	reg [31:0] expected_base = 32'h227e9000;
	reg [15:0] route_epoch = 16'd11;
	reg [15:0] route_flags = 16'h0005;
	reg frame_marker = 1'b0;
	reg reset_out = 1'b0;
	reg [27:0] address = 28'd0;
	reg [7:0] burstcount = 8'd128;
	reg waitrequest = 1'b0;
	reg readdatavalid = 1'b0;
	reg read = 1'b0;
	reg write = 1'b0;
	reg [15:0] byteenable = 16'hffff;
	wire fault_toggle;
	wire [7:0] fault_trigger;
	wire snapshot_ack;
	wire [239:0] payload;
	wire address_fault_toggle;
	wire [7:0] address_fault_trigger;

	mister_magik_video_diagnostics_avalon dut (
		.clk_100m(clk_100m), .monitor_armed_async(armed),
		.snapshot_request_toggle_async(snapshot_request),
		.diagnostic_generation_async(generation),
		.route_context_toggle_async(route_context), .expected_base_async(expected_base),
		.expected_route_epoch_async(route_epoch),
		.expected_route_flags_async(route_flags), .frame_marker_async(frame_marker),
		.reset_out_async(reset_out), .vbuf_address(address),
		.vbuf_burstcount(burstcount), .vbuf_waitrequest(waitrequest),
		.vbuf_readdatavalid(readdatavalid), .vbuf_read(read), .vbuf_write(write),
		.vbuf_byteenable(byteenable), .fault_toggle(fault_toggle),
		.fault_trigger(fault_trigger), .snapshot_ack_toggle(snapshot_ack),
		.snapshot_payload(payload)
	);

	mister_magik_video_diagnostics_avalon address_dut (
		.clk_100m(clk_100m), .monitor_armed_async(armed_address),
		.snapshot_request_toggle_async(1'b0),
		.diagnostic_generation_async(generation),
		.route_context_toggle_async(route_context), .expected_base_async(expected_base),
		.expected_route_epoch_async(route_epoch),
		.expected_route_flags_async(route_flags), .frame_marker_async(frame_marker),
		.reset_out_async(reset_out), .vbuf_address(address),
		.vbuf_burstcount(burstcount), .vbuf_waitrequest(waitrequest),
		.vbuf_readdatavalid(readdatavalid), .vbuf_read(read), .vbuf_write(write),
		.vbuf_byteenable(byteenable), .fault_toggle(address_fault_toggle),
		.fault_trigger(address_fault_trigger), .snapshot_ack_toggle(), .snapshot_payload()
	);

	function automatic [15:0] word_at;
		input integer index;
		begin word_at = payload[index*16 +: 16]; end
	endfunction

	task automatic accepted_read;
		input [27:0] read_address;
		input [7:0] read_burst;
		integer beat;
		begin
			@(negedge clk_100m);
			address = read_address;
			burstcount = read_burst;
			read = 1'b1;
			@(negedge clk_100m);
			read = 1'b0;
			for(beat = 0; beat < read_burst; beat = beat + 1) begin
				readdatavalid = 1'b1;
				@(negedge clk_100m);
			end
			readdatavalid = 1'b0;
		end
	endtask

	reg initial_fault;
	reg [239:0] frozen_payload;
	initial begin
		repeat(4) @(negedge clk_100m);
		route_context = ~route_context;
		armed = 1'b1;
		repeat(4) @(negedge clk_100m);

		// A base flip and first read can arrive together. The old slot must not
		// be used to reject the new route while its observer mailbox settles.
		@(negedge clk_100m);
		expected_base = 32'h22fd2000;
		route_epoch = route_epoch + 1'd1;
		route_context = ~route_context;
		address = 28'h22fd200;
		read = 1'b1;
		@(negedge clk_100m);
		read = 1'b0;
		repeat(128) begin
			readdatavalid = 1'b1;
			@(negedge clk_100m);
		end
		readdatavalid = 1'b0;
		repeat(4) @(negedge clk_100m);
		if(fault_toggle || fault_trigger != 0)
			$fatal(1, "same-cycle route flip and legal read faulted");

		// A legal 128-beat read at the scaler-side Avalon boundary must not fault.
		accepted_read(expected_base[31:4], 8'd128);
		if(fault_toggle || fault_trigger != 0) $fatal(1, "legal read faulted");

		// Lifetime traffic counters are evidence, not fault predicates. Legal
		// scanout must continue after both compact counters saturate.
		@(negedge clk_100m);
		dut.accepted_bursts = 16'hfffe;
		dut.returned_beats = 16'hfffe;
		accepted_read(expected_base[31:4] + 28'd1, 8'd128);
		accepted_read(expected_base[31:4] + 28'd2, 8'd128);
		if(fault_toggle || fault_trigger != 0)
			$fatal(1, "legal saturated traffic counter faulted");
		if(dut.accepted_bursts != 16'hffff || dut.returned_beats != 16'hffff)
			$fatal(1, "traffic counters did not saturate");

		// The first invalid burst freezes the evidence. Later faults cannot replace it.
		accepted_read(expected_base[31:4] + 28'd128, 8'd4);
		repeat(4) @(negedge clk_100m);
		if(fault_trigger != 8'd6) $fatal(1, "bad burst was not classified");
		initial_fault = fault_toggle;
		@(negedge clk_100m);
		address = 28'h0000001;
		burstcount = 8'd128;
		read = 1'b1;
		@(negedge clk_100m);
		read = 1'b0;
		repeat(3) @(negedge clk_100m);
		if(fault_toggle != initial_fault || fault_trigger != 8'd6)
			$fatal(1, "first-fault evidence changed");

		snapshot_request = ~snapshot_request;
		repeat(5) @(negedge clk_100m);
		if(snapshot_ack != snapshot_request) $fatal(1, "snapshot mailbox did not acknowledge");
		if(word_at(0) != MAGIK_VIDEO_DIAGNOSTICS_SCHEMA ||
		   word_at(2) != 16'd6 || word_at(3) != generation)
			$fatal(1, "snapshot identity mismatch schema=%h trigger=%h generation=%h expected=%h",
				word_at(0), word_at(2), word_at(3), generation);
		if(word_at(4) != route_epoch || word_at(5) != route_flags)
			$fatal(1, "route context mismatch");
		if((word_at(1) & 16'h000c) != 16'h000c)
			$fatal(1, "snapshot state flags mismatch");
		if((word_at(14) & 16'h0002) == 0)
			$fatal(1, "burst fault context missing");
		frozen_payload = payload;
		frame_marker = ~frame_marker;
		waitrequest = 1'b1;
		read = 1'b1;
		repeat(6) @(negedge clk_100m);
		if(payload !== frozen_payload) $fatal(1, "acknowledged Avalon mailbox changed");
		waitrequest = 1'b0;
		read = 1'b0;

		// Once the new route is coherent, an old-slot read must be diagnosed.
		armed_address = 1'b1;
		repeat(4) @(negedge clk_100m);
		accepted_read(28'h227e900, 8'd128);
		repeat(4) @(negedge clk_100m);
		if(address_fault_trigger != 8'd5 || !address_fault_toggle)
			$fatal(1, "stable old-slot address was not diagnosed");
		$display("video diagnostics Avalon tests passed");
		$finish;
	end
endmodule

`default_nettype wire
