// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

`timescale 1ns/1ps
`default_nettype none

module tb_mister_magik_vblank_latch;
	`include "mister_magik_latch_protocol.svh"
	localparam [7:0] SET_LATCH = MAGIK_UIO_SET_FBUF_LATCH;
	localparam [7:0] GET_LATCH = MAGIK_UIO_GET_FBUF_LATCH;
	localparam [7:0] GET_CAPS = MAGIK_UIO_GET_FBUF_LATCH_CAPS;

	reg clk_sys = 1'b0;
	reg hdmi_vbl = 1'b0;
	reg crt_vblank = 1'b0;
	reg cmd_start = 1'b0;
	reg cmd_data = 1'b0;
	reg [7:0] cmd_id = 8'd0;
	reg [3:0] word_index = 4'd0;
	reg [15:0] data_in = 16'd0;
	reg active_lfb_en = 1'b0;
	reg [31:0] active_lfb_base = 32'd0;
	reg [11:0] active_lfb_width = 12'd0;
	reg [11:0] active_lfb_height = 12'd0;
	reg [13:0] active_lfb_stride = 14'd0;
	reg [15:0] reader_flags = 16'd0;
	reg [15:0] reader_underrun_count = 16'd0;
	reg [15:0] reader_timeout_count = 16'd0;

	wire response_valid;
	wire [15:0] response_data;
	wire apply;
	wire apply_hdmi;
	wire apply_crt;
	wire route_en;
	wire route_flt;
	wire [5:0] route_fmt;
	wire [11:0] route_width;
	wire [11:0] route_height;
	wire [11:0] route_hmin;
	wire [11:0] route_hmax;
	wire [11:0] route_vmin;
	wire [11:0] route_vmax;
	wire [31:0] route_base;
	wire [13:0] route_stride;
	wire pending;
	wire [15:0] pending_seq;
	wire [15:0] active_seq;
	wire [15:0] post_count;
	wire [15:0] flip_count;
	wire [15:0] drop_count;
	wire [1:0] requested_route;
	wire [1:0] active_route;

	integer apply_count = 0;
	reg [7:0] requirement_coverage = 8'd0;
	reg [15:0] captured_seq = 16'd0;
	reg [31:0] captured_base = 32'd0;
	reg [11:0] captured_width = 12'd0;
	reg [11:0] captured_height = 12'd0;
	reg [11:0] captured_hmin = 12'd0;
	reg [11:0] captured_hmax = 12'd0;
	reg [11:0] captured_vmin = 12'd0;
	reg [11:0] captured_vmax = 12'd0;
	reg [13:0] captured_stride = 14'd0;
	reg [7:0] captured_mode = 8'd0;

	mister_magik_vblank_latch dut (
		.clk_sys(clk_sys),
		.hdmi_vbl(hdmi_vbl),
		.crt_vblank(crt_vblank),
		.cmd_start(cmd_start),
		.cmd_data(cmd_data),
		.cmd_id(cmd_id),
		.word_index(word_index),
		.data_in(data_in),
		.active_lfb_en(active_lfb_en),
		.active_lfb_base(active_lfb_base),
		.active_lfb_width(active_lfb_width),
		.active_lfb_height(active_lfb_height),
		.active_lfb_stride(active_lfb_stride),
		.reader_flags(reader_flags),
		.reader_underrun_count(reader_underrun_count),
		.reader_timeout_count(reader_timeout_count),
		.response_valid(response_valid),
		.response_data(response_data),
		.apply(apply),
		.apply_hdmi(apply_hdmi),
		.apply_crt(apply_crt),
		.route_en(route_en),
		.route_flt(route_flt),
		.route_fmt(route_fmt),
		.route_width(route_width),
		.route_height(route_height),
		.route_hmin(route_hmin),
		.route_hmax(route_hmax),
		.route_vmin(route_vmin),
		.route_vmax(route_vmax),
		.route_base(route_base),
		.route_stride(route_stride),
		.pending(pending),
		.pending_seq(pending_seq),
		.active_seq(active_seq),
		.post_count(post_count),
		.flip_count(flip_count),
		.drop_count(drop_count),
		.requested_route(requested_route),
		.active_route(active_route)
	);

	always #5 clk_sys = ~clk_sys;

	always @(posedge clk_sys) begin
		if(apply) begin
			apply_count = apply_count + 1;
			captured_seq = pending_seq;
			captured_base = route_base;
			captured_width = route_width;
			captured_height = route_height;
			captured_hmin = route_hmin;
			captured_hmax = route_hmax;
			captured_vmin = route_vmin;
			captured_vmax = route_vmax;
			captured_stride = route_stride;
			captured_mode = {route_en, route_flt, route_fmt};
		end
	end

	task automatic fail(input [8*96-1:0] message);
		begin
			$display("FAIL: %0s", message);
			$fatal(1);
		end
	endtask

	task automatic expect16(
		input [15:0] actual,
		input [15:0] expected,
		input [8*96-1:0] message
	);
		begin
			if(actual !== expected) begin
				$display("FAIL: %0s: got %04x expected %04x", message, actual, expected);
				$fatal(1);
			end
		end
	endtask

	task automatic expect_true(input condition, input [8*96-1:0] message);
		begin
			if(condition !== 1'b1) fail(message);
		end
	endtask

	task automatic idle_cycles(input integer count);
		integer i;
		begin
			for(i = 0; i < count; i = i + 1) @(posedge clk_sys);
			#1;
		end
	endtask

	task automatic check_ack(input [7:0] command, input [15:0] expected);
		begin
			@(negedge clk_sys);
			cmd_id = command;
			cmd_start = 1'b1;
			#1;
			expect_true(response_valid, "recognized command must acknowledge");
			expect16(response_data, expected, "command acknowledgement magic");
			@(negedge clk_sys);
			cmd_start = 1'b0;
		end
	endtask

	task automatic check_unrelated_ack;
		begin
			@(negedge clk_sys);
			cmd_id = 8'h56;
			cmd_start = 1'b1;
			#1;
			if(response_valid !== 1'b0) fail("unrelated command acknowledged");
			@(negedge clk_sys);
			cmd_start = 1'b0;
		end
	endtask

	task automatic send_word(
		input [7:0] command,
		input [3:0] index,
		input [15:0] value
	);
		begin
			@(negedge clk_sys);
			cmd_id = command;
			word_index = index;
			data_in = value;
			cmd_data = 1'b1;
			@(posedge clk_sys);
			#1;
			cmd_data = 1'b0;
		end
	endtask

	task automatic expect_status(input [3:0] index, input [15:0] expected);
		begin
			@(negedge clk_sys);
			cmd_id = GET_LATCH;
			word_index = index;
			cmd_data = 1'b1;
			#1;
			expect_true(response_valid, "status word must be valid");
			expect16(response_data, expected, "status word mismatch");
			cmd_data = 1'b0;
		end
	endtask

	task automatic expect_caps(input [3:0] index, input [15:0] expected);
		begin
			@(negedge clk_sys);
			cmd_id = GET_CAPS;
			word_index = index;
			cmd_data = 1'b1;
			#1;
			expect_true(response_valid, "capability word must be valid");
			expect16(response_data, expected, "capability word mismatch");
			cmd_data = 1'b0;
		end
	endtask

	task automatic send_route(
		input [15:0] mode_value,
		input [31:0] base,
		input [11:0] width,
		input [11:0] height,
		input [11:0] hmin,
		input [11:0] hmax,
		input [11:0] vmin,
		input [11:0] vmax,
		input [13:0] stride,
		input [15:0] seq_value
	);
		begin
			send_word(SET_LATCH, 4'd0, mode_value);
			send_word(SET_LATCH, 4'd1, base[15:0]);
			send_word(SET_LATCH, 4'd2, base[31:16]);
			send_word(SET_LATCH, 4'd3, {4'd0, width});
			send_word(SET_LATCH, 4'd4, {4'd0, height});
			send_word(SET_LATCH, 4'd5, {4'd0, hmin});
			send_word(SET_LATCH, 4'd6, {4'd0, hmax});
			send_word(SET_LATCH, 4'd7, {4'd0, vmin});
			send_word(SET_LATCH, 4'd8, {4'd0, vmax});
			send_word(SET_LATCH, 4'd9, {2'd0, stride});
			send_word(SET_LATCH, 4'd10, seq_value);
		end
	endtask

	task automatic raise_vblank_and_wait_for_flip(
		input [15:0] expected_flip,
		input integer expected_apply_count
	);
		integer i;
		reg found;
		begin
			@(negedge clk_sys);
			hdmi_vbl = 1'b1;
			found = 1'b0;
			for(i = 0; i < 8; i = i + 1) begin
				@(posedge clk_sys);
				#1;
				if(flip_count == expected_flip) begin
					found = 1'b1;
					i = 8;
				end
			end
			if(!found) fail("bounded wait for vblank flip expired");
			if(apply_count != expected_apply_count) fail("apply pulse count mismatch");
		end
	endtask

	task automatic lower_vblank;
		begin
			@(negedge clk_sys);
			hdmi_vbl = 1'b0;
			idle_cycles(4);
		end
	endtask

	initial begin
		idle_cycles(2);

		expect16(active_seq, 16'd0, "power-up active sequence");
		expect16(pending_seq, 16'd0, "power-up pending sequence");
		expect16(post_count, 16'd0, "power-up post count");
		expect16(flip_count, 16'd0, "power-up flip count");
		expect16(drop_count, 16'd0, "power-up drop count");
		if(pending || route_en || route_flt || (route_fmt != 0) ||
		   (route_base != 0) || (route_width != 0) || (route_height != 0) ||
		   (route_hmin != 0) || (route_hmax != 0) || (route_vmin != 0) ||
		   (route_vmax != 0) || (route_stride != 0))
			fail("power-up route state is not zero");

		check_ack(SET_LATCH, 16'h4D47);
		check_ack(GET_LATCH, 16'h4D48);
		check_ack(GET_CAPS, 16'h4D49);
		expect_caps(4'd0, 16'd3);
		expect_caps(4'd1, 16'h0007);
		expect_caps(4'd2, 16'd1366);
		expect_caps(4'd3, 16'd768);
		expect_caps(4'd4, 16'd2736);
		expect_caps(4'd5, 16'h0003);
		expect_caps(4'd6, 16'd1);
		expect_caps(4'd15, 16'd0);
		requirement_coverage[0] = 1'b1; // LATCH-001
		check_unrelated_ack();

		// Partial and unrelated payloads can stage data but cannot post a route.
		send_word(SET_LATCH, 4'd0, 16'hC02A);
		send_word(SET_LATCH, 4'd1, 16'h5678);
		send_word(SET_LATCH, 4'd15, 16'hFFFF);
		send_word(8'h56, 4'd10, 16'h9999);
		idle_cycles(4);
		if(pending || (post_count != 0) || (flip_count != 0) || (apply_count != 0))
			fail("partial, out-of-range, or unrelated word posted a route");
		requirement_coverage[2] = 1'b1; // LATCH-003

		// Exercise every payload word and prove no edge means no application.
		send_route(16'hC02A, 32'h12345678, 12'd960, 12'd540,
		           12'd11, 12'd970, 12'd22, 12'd561, 14'd1920, 16'h0010);
		expect_true(pending, "sequence word must set pending");
		expect16(pending_seq, 16'h0010, "pending sequence after post");
		expect16(post_count, 16'd1, "post count after first post");
		expect16(drop_count, 16'd0, "first post must not drop");
		if({route_en, route_flt, route_fmt} !== 8'hEA ||
		   route_base !== 32'h12345678 || route_width !== 12'd960 ||
		   route_height !== 12'd540 || route_hmin !== 12'd11 ||
		   route_hmax !== 12'd970 || route_vmin !== 12'd22 ||
		   route_vmax !== 12'd561 || route_stride !== 14'd1920)
			fail("staged route fields mismatch");
		requirement_coverage[1] = 1'b1; // LATCH-002
		idle_cycles(6);
		if((flip_count != 0) || (apply_count != 0)) fail("route applied without vblank edge");

		raise_vblank_and_wait_for_flip(16'd1, 1);
		expect16(active_seq, 16'h0010, "active sequence after first flip");
		if(pending) fail("pending did not clear after flip");
		if(captured_seq !== 16'h0010 || captured_mode !== 8'hEA ||
		   captured_base !== 32'h12345678 || captured_width !== 12'd960 ||
		   captured_height !== 12'd540 || captured_hmin !== 12'd11 ||
		   captured_hmax !== 12'd970 || captured_vmin !== 12'd22 ||
		   captured_vmax !== 12'd561 || captured_stride !== 14'd1920)
			fail("route was not captured atomically on apply");
		requirement_coverage[3] = 1'b1; // LATCH-004

		// Keeping vblank high must not create another apply.
		idle_cycles(5);
		if((flip_count != 1) || (apply_count != 1)) fail("level or falling vblank applied route");
		requirement_coverage[7] = 1'b1; // LATCH-008

		// A complete replacement while pending wins and accounts one dropped post.
		// Stage it while vblank remains high, then prove the falling edge is inert.
		send_route(16'h8021, 32'h20001000, 12'd1280, 12'd720,
		           12'd1, 12'd1280, 12'd2, 12'd721, 14'd2560, 16'h0020);
		send_route(16'h4022, 32'h30002000, 12'd640, 12'd480,
		           12'd3, 12'd642, 12'd4, 12'd483, 14'd1280, 16'h0021);
		expect16(post_count, 16'd3, "post count after replacement");
		expect16(drop_count, 16'd1, "replacement drop count");
		expect16(pending_seq, 16'h0021, "replacement pending sequence");
		if((flip_count != 1) || (apply_count != 1)) fail("high vblank level applied replacement");
		lower_vblank();
		if(!pending || (flip_count != 1) || (apply_count != 1))
			fail("falling vblank edge applied replacement");
		raise_vblank_and_wait_for_flip(16'd2, 2);
		expect16(active_seq, 16'h0021, "replacement active sequence");
		if(captured_seq !== 16'h0021 || captured_mode !== 8'h62 ||
		   captured_base !== 32'h30002000 || captured_width !== 12'd640 ||
		   captured_height !== 12'd480 || captured_stride !== 14'd1280)
			fail("replacement route did not win atomically");
		requirement_coverage[4] = 1'b1; // LATCH-005
		lower_vblank();

		// Exact status layout, including externally-owned active route fields.
		active_lfb_en = 1'b1;
		active_lfb_base = 32'h89ABCDEF;
		active_lfb_width = 12'd1280;
		active_lfb_height = 12'd720;
		active_lfb_stride = 14'd2560;
		expect_status(4'd0, 16'h0021);
		expect_status(4'd1, 16'h0021);
		expect_status(4'd2, 16'h0001);
		expect_status(4'd3, 16'd2);
		expect_status(4'd4, 16'd3);
		expect_status(4'd5, 16'd1);
		expect_status(4'd6, 16'hCDEF);
		expect_status(4'd7, 16'h89AB);
		expect_status(4'd8, 16'd1280);
		expect_status(4'd9, 16'd720);
		expect_status(4'd10, 16'd2560);
		expect_status(4'd11, 16'd0);
		expect_status(4'd12, 16'd0);
		reader_flags = 16'h000f;
		reader_underrun_count = 16'd7;
		reader_timeout_count = 16'd8;
		expect_status(4'd13, 16'h000f);
		expect_status(4'd14, 16'd7);
		expect_status(4'd15, 16'd8);
		requirement_coverage[5] = 1'b1; // LATCH-006

		// Force the natural 16-bit wrap boundaries, then exercise them normally.
		@(negedge clk_sys);
		dut.post_count = 16'hFFFF;
		dut.flip_count = 16'hFFFF;
		dut.drop_count = 16'hFFFF;
		send_route(16'h8001, 32'h40000000, 12'd320, 12'd240,
		           12'd0, 12'd319, 12'd0, 12'd239, 14'd640, 16'hFFFF);
		expect16(post_count, 16'h0000, "post counter wrap");
		send_word(SET_LATCH, 4'd10, 16'hFFFF);
		expect16(post_count, 16'h0001, "post count after wrapped replacement");
		expect16(drop_count, 16'h0000, "drop counter wrap");
		expect16(pending_seq, 16'hFFFF, "maximum pending sequence");
		raise_vblank_and_wait_for_flip(16'h0000, 3);
		expect16(flip_count, 16'h0000, "flip counter wrap");
		expect16(active_seq, 16'hFFFF, "maximum active sequence");
		lower_vblank();
		send_route(16'h0000, 32'h00000000, 12'd0, 12'd0,
		           12'd0, 12'd0, 12'd0, 12'd0, 14'd0, 16'h0000);
		expect16(pending_seq, 16'h0000, "sequence wrap to zero");
		raise_vblank_and_wait_for_flip(16'h0001, 4);
		expect16(active_seq, 16'h0000, "active sequence wrap");
		requirement_coverage[6] = 1'b1; // LATCH-007
		lower_vblank();

		// CRT requests ignore HDMI blank and apply only at the CRT boundary.
		send_route(16'hC06A, 32'h227E9000, 12'd640, 12'd240,
		           12'd0, 12'd639, 12'd0, 12'd239, 14'd1280, 16'h0042);
		expect16({14'd0, requested_route}, 16'd1, "requested CRT route");
		@(negedge clk_sys); hdmi_vbl = 1'b1; idle_cycles(5);
		if((flip_count != 1) || (apply_count != 4) || !pending)
			fail("CRT request applied on HDMI boundary");
		lower_vblank();
		@(negedge clk_sys); crt_vblank = 1'b1; idle_cycles(5);
		if((flip_count != 2) || (apply_count != 5) || active_route != 1)
			fail("CRT request did not apply at CRT boundary");
		@(negedge clk_sys); crt_vblank = 1'b0; idle_cycles(4);

		if(requirement_coverage !== 8'hFF) fail("not all RTL requirement coverpoints hit");
		$display("COVER LATCH-001..LATCH-008 all RTL requirements hit");

		$display("PASS: mister_magik_vblank_latch protocol and vblank semantics");
		$finish;
	end

	initial begin
		#20000;
		fail("global simulation timeout");
	end

endmodule

`default_nettype wire
