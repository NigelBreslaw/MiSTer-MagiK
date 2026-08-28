// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

`timescale 1ns/1ps
`default_nettype none

// Thin sys_top register harness around the exact production bridge. The checker
// separately applies the production patch and verifies its complete bridge
// mapping, response routing, and authoritative LFB apply bundle.
module mister_magik_sys_top_latch_path (
	input wire clk_sys,
	input wire hdmi_vbl,
	input wire io_uio,
	input wire io_strobe,
	input wire [15:0] io_din
);
	reg [15:0] io_dout_sys = 16'd0;
	reg LFB_EN = 1'b0;
	reg LFB_FLT = 1'b0;
	reg [5:0] LFB_FMT = 6'd0;
	reg [11:0] LFB_WIDTH = 12'd0;
	reg [11:0] LFB_HEIGHT = 12'd0;
	reg [11:0] LFB_HMIN = 12'd0;
	reg [11:0] LFB_HMAX = 12'd0;
	reg [11:0] LFB_VMIN = 12'd0;
	reg [11:0] LFB_VMAX = 12'd0;
	reg [31:0] LFB_BASE = 32'd0;
	reg [13:0] LFB_STRIDE = 14'd0;

	wire magik_response_valid;
	wire [15:0] magik_response_data;
	wire magik_diag_response_valid;
	wire [15:0] magik_diag_response_data;
	wire magik_lfb_apply;
	wire magik_lfb_apply_accepted;
	wire legacy_lfb_write;
	wire [3:0] active_word_index;
	wire magik_lfb_en;
	wire magik_lfb_flt;
	wire [5:0] magik_lfb_fmt;
	wire [11:0] magik_lfb_width;
	wire [11:0] magik_lfb_height;
	wire [11:0] magik_lfb_hmin;
	wire [11:0] magik_lfb_hmax;
	wire [11:0] magik_lfb_vmin;
	wire [11:0] magik_lfb_vmax;
	wire [31:0] magik_lfb_base;
	wire [13:0] magik_lfb_stride;
	wire magik_lfb_pending;
	wire [15:0] magik_lfb_pending_seq;
	wire [15:0] magik_lfb_active_seq;
	wire [15:0] magik_lfb_post_count;
	wire [15:0] magik_lfb_flip_count;
	wire [15:0] magik_lfb_drop_count;
	wire [15:0] magik_lfb_reject_count;
	wire [15:0] magik_lfb_active_route_epoch;

	mister_magik_latch_sys_top_bridge bridge (
		.clk_sys(clk_sys),
		.hdmi_vbl(hdmi_vbl),
		.io_uio(io_uio),
		.io_strobe(io_strobe),
		.io_din(io_din),
		.active_lfb_en(LFB_EN),
		.active_lfb_base(LFB_BASE),
		.active_lfb_width(LFB_WIDTH),
		.active_lfb_height(LFB_HEIGHT),
		.active_lfb_stride(LFB_STRIDE),
		.response_valid(magik_response_valid),
		.response_data(magik_response_data),
		.apply(magik_lfb_apply),
		.apply_accepted(magik_lfb_apply_accepted),
		.legacy_write(legacy_lfb_write),
		.active_word_index(active_word_index),
		.route_en(magik_lfb_en),
		.route_flt(magik_lfb_flt),
		.route_fmt(magik_lfb_fmt),
		.route_width(magik_lfb_width),
		.route_height(magik_lfb_height),
		.route_hmin(magik_lfb_hmin),
		.route_hmax(magik_lfb_hmax),
		.route_vmin(magik_lfb_vmin),
		.route_vmax(magik_lfb_vmax),
		.route_base(magik_lfb_base),
		.route_stride(magik_lfb_stride),
		.pending(magik_lfb_pending),
		.pending_seq(magik_lfb_pending_seq),
		.active_seq(magik_lfb_active_seq),
		.post_count(magik_lfb_post_count),
		.flip_count(magik_lfb_flip_count),
		.drop_count(magik_lfb_drop_count),
		.reject_count(magik_lfb_reject_count),
		.active_route_epoch(magik_lfb_active_route_epoch)
	);

	mister_magik_scaler_fetch_liveness_state diagnostic (
		.clk_100m(clk_sys),
		.clk_sys(clk_sys),
		.reset_req(1'b0),
		.vbuf_address(28'd0),
		.vbuf_burstcount(8'd128),
		.vbuf_waitrequest(1'b0),
		.vbuf_readdatavalid(1'b0),
		.vbuf_read(1'b0),
		.io_uio(io_uio),
		.io_strobe(io_strobe),
		.io_din(io_din),
		.response_valid(magik_diag_response_valid),
		.response_data(magik_diag_response_data)
	);

	always @(posedge clk_sys) begin
		if(magik_lfb_apply_accepted) begin
			LFB_EN <= magik_lfb_en;
			LFB_FLT <= magik_lfb_flt;
			LFB_FMT <= magik_lfb_fmt;
			LFB_WIDTH <= magik_lfb_width;
			LFB_HEIGHT <= magik_lfb_height;
			LFB_HMIN <= magik_lfb_hmin;
			LFB_HMAX <= magik_lfb_hmax;
			LFB_VMIN <= magik_lfb_vmin;
			LFB_VMAX <= magik_lfb_vmax;
			LFB_BASE <= magik_lfb_base;
			LFB_STRIDE <= magik_lfb_stride;
		end
		if(legacy_lfb_write) begin
			case(active_word_index)
				4'd0: {LFB_EN,LFB_FLT,LFB_FMT} <=
					{io_din[15], io_din[14], io_din[5:0]};
				4'd1: LFB_BASE[15:0] <= io_din;
				4'd2: LFB_BASE[31:16] <= io_din;
				4'd3: LFB_WIDTH <= io_din[11:0];
				4'd4: LFB_HEIGHT <= io_din[11:0];
				4'd5: LFB_HMIN <= io_din[11:0];
				4'd6: LFB_HMAX <= io_din[11:0];
				4'd7: LFB_VMIN <= io_din[11:0];
				4'd8: LFB_VMAX <= io_din[11:0];
				4'd9: LFB_STRIDE <= io_din[13:0];
				default: begin end
			endcase
		end
		if(!io_uio) io_dout_sys <= 16'd0;
		else if(io_strobe) begin
			io_dout_sys <= 16'd0;
			if(!bridge.has_command && (io_din[7:0] == 8'h2f))
				io_dout_sys <= 16'd1;
			if(magik_response_valid)
				io_dout_sys <= magik_response_data;
			if(magik_diag_response_valid)
				io_dout_sys <= magik_diag_response_data;
		end
	end
endmodule

module tb_mister_magik_sys_top_integration;
	`include "mister_magik_latch_protocol.svh"
	`include "mister_magik_video_diagnostics_protocol.svh"

	reg test_clk = 1'b0;
	reg test_vblank = 1'b0;
	reg test_io_uio = 1'b0;
	reg test_io_strobe = 1'b0;
	reg [15:0] test_io_din = 16'd0;
	integer index;

	mister_magik_sys_top_latch_path dut (
		.clk_sys(test_clk),
		.hdmi_vbl(test_vblank),
		.io_uio(test_io_uio),
		.io_strobe(test_io_strobe),
		.io_din(test_io_din)
	);

	always #5 test_clk = ~test_clk;

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

	function automatic [15:0] crc_byte;
		input [15:0] current;
		input [7:0] value;
		integer bit_index;
		reg [15:0] next;
		begin
			next = current ^ {value, 8'h00};
			for(bit_index = 0; bit_index < 8; bit_index = bit_index + 1) begin
				if(next[15]) next = (next << 1) ^ 16'h1021;
				else next = next << 1;
			end
			crc_byte = next;
		end
	endfunction

	function automatic [15:0] crc_word;
		input [15:0] current;
		input [15:0] value;
		begin
			crc_word = crc_byte(crc_byte(current, value[15:8]), value[7:0]);
		end
	endfunction

	function automatic [15:0] crc_header;
		input [7:0] command;
		input [15:0] count;
		reg [15:0] next;
		begin
			next = crc_word(16'hffff, {8'd0, command});
			next = crc_word(next, MAGIK_FBUF_PROTOCOL_VERSION);
			crc_header = crc_word(next, count);
		end
	endfunction

	function automatic [15:0] golden_set_word;
		input [3:0] word;
		begin
			case(word)
				4'd0: golden_set_word = MAGIK_GOLDEN_SET_V5_0;
				4'd1: golden_set_word = MAGIK_GOLDEN_SET_V5_1;
				4'd2: golden_set_word = MAGIK_GOLDEN_SET_V5_2;
				4'd3: golden_set_word = MAGIK_GOLDEN_SET_V5_3;
				4'd4: golden_set_word = MAGIK_GOLDEN_SET_V5_4;
				4'd5: golden_set_word = MAGIK_GOLDEN_SET_V5_5;
				4'd6: golden_set_word = MAGIK_GOLDEN_SET_V5_6;
				4'd7: golden_set_word = MAGIK_GOLDEN_SET_V5_7;
				4'd8: golden_set_word = MAGIK_GOLDEN_SET_V5_8;
				4'd9: golden_set_word = MAGIK_GOLDEN_SET_V5_9;
				4'd10: golden_set_word = MAGIK_GOLDEN_SET_V5_10;
				default: golden_set_word = MAGIK_GOLDEN_SET_V5_CRC;
			endcase
		end
	endfunction

	task automatic begin_command(input [7:0] command, input [15:0] magic);
		begin
			@(negedge test_clk);
			test_io_uio = 1'b1;
			test_io_strobe = 1'b1;
			test_io_din = {8'd0, command};
			@(posedge test_clk);
			#1;
			expect16(dut.io_dout_sys, magic, "sys_top command acknowledgement");
			test_io_strobe = 1'b0;
		end
	endtask

	task automatic transfer_word(input [15:0] word, output [15:0] response);
		begin
			@(negedge test_clk);
			test_io_din = word;
			test_io_strobe = 1'b1;
			@(posedge test_clk);
			#1;
			response = dut.io_dout_sys;
			test_io_strobe = 1'b0;
		end
	endtask

	task automatic end_command;
		begin
			@(negedge test_clk);
			test_io_uio = 1'b0;
			test_io_strobe = 1'b0;
			@(posedge test_clk);
			#1;
		end
	endtask

	initial begin
		reg [15:0] response;
		reg [15:0] telemetry [0:10];
		reg [15:0] telemetry_crc;
		repeat(3) @(posedge test_clk);
		end_command();
		// Exercise the production bridge's selected-but-idle parser state.
		@(negedge test_clk);
		test_io_uio = 1'b1;
		test_io_strobe = 1'b0;
		@(posedge test_clk);
		#1;
		end_command();

		begin_command(MAGIK_UIO_GET_FBUF_LATCH_CAPS, MAGIK_FBUF_CAPS_MAGIC);
		transfer_word(16'd0, response);
		expect16(response, MAGIK_FBUF_PROTOCOL_VERSION, "sys_top caps version");
		transfer_word(16'd0, response);
		expect16(response, MAGIK_FBUF_CAPS_FLAGS, "sys_top caps flags");
		for(index = 2; index < 5; index = index + 1)
			transfer_word(16'd0, response);
		transfer_word(16'd0, response);
		expect16(response, MAGIK_GOLDEN_CAPS_V5_CRC, "sys_top caps CRC");
		end_command();

		begin_command(MAGIK_UIO_SET_FBUF_LATCH, MAGIK_FBUF_LATCH_MAGIC);
		for(index = 0; index < 12; index = index + 1)
			transfer_word(golden_set_word(index[3:0]), response);
		end_command();
		if(!dut.magik_lfb_pending) fail("sys_top SET did not commit pending route");

		@(negedge test_clk);
		test_vblank = 1'b1;
		repeat(5) @(posedge test_clk);
		#1;
		expect16(dut.LFB_BASE[15:0], 16'h9000, "sys_top accepted base low");
		expect16(dut.LFB_BASE[31:16], 16'h227e, "sys_top accepted base high");
		expect16({4'd0, dut.LFB_HEIGHT}, 16'd540, "sys_top accepted height");
		expect16(dut.magik_lfb_active_seq, 16'h002b, "sys_top accepted sequence");
		@(negedge test_clk);
		test_vblank = 1'b0;
		repeat(4) @(posedge test_clk);

		begin_command(
			MAGIK_UIO_GET_FBUF_PRESENTATION_TELEMETRY,
			MAGIK_FBUF_PRESENTATION_TELEMETRY_MAGIC
		);
		for(index = 0; index < 11; index = index + 1)
			transfer_word(16'd0, telemetry[index]);
		end_command();
		expect16(telemetry[0], 16'd1, "sys_top telemetry owned count");
		expect16(telemetry[2], 16'd1, "sys_top telemetry presented count");
		expect16(telemetry[4], 16'd0, "sys_top telemetry repeated count");
		expect16(telemetry[6], 16'd0, "sys_top telemetry ownership loss count");
		expect16(telemetry[8], 16'h002b, "sys_top telemetry active sequence");
		expect16(telemetry[9], 16'h0009, "sys_top telemetry live flags");
		telemetry_crc = crc_header(MAGIK_UIO_GET_FBUF_PRESENTATION_TELEMETRY, 16'd10);
		for(index = 0; index < 10; index = index + 1)
			telemetry_crc = crc_word(telemetry_crc, telemetry[index]);
		expect16(telemetry[10], telemetry_crc, "sys_top telemetry CRC");

		// Only the liveness scaler-fetch record is supported. Legacy diagnostics
		// remain explicitly unsupported and cannot disturb latch-v5 state.
		for(index = 8'h60; index <= 8'h67; index = index + 1) begin
			begin_command(index[7:0], 16'd0);
			end_command();
		end
		repeat(20) @(posedge test_clk);
		begin_command(MAGIK_UIO_GET_SCALER_FETCH_LIVENESS_STATE,
			MAGIK_SCALER_FETCH_LIVENESS_STATE_MAGIC);
		for(index = 0; index < MAGIK_SCALER_FETCH_LIVENESS_STATE_WORDS;
			index = index + 1)
			transfer_word(16'd0, response);
		end_command();
		expect16(dut.magik_lfb_active_seq, 16'h002b,
			"diagnostic read changed latch-v5 active sequence");

		// Post another route, then collide its vblank apply with a real 0x2f
		// payload edge. The production legacy-write expression must win.
		begin_command(MAGIK_UIO_SET_FBUF_LATCH, MAGIK_FBUF_LATCH_MAGIC);
		for(index = 0; index < 12; index = index + 1)
			transfer_word(golden_set_word(index[3:0]), response);
		end_command();
		if(!dut.magik_lfb_pending) fail("new route is not pending");

		begin_command(8'h2f, 16'd1);
		@(negedge test_clk);
		test_vblank = 1'b1;
		while(!dut.magik_lfb_apply) @(negedge test_clk);
		test_io_din = 16'h0000;
		test_io_strobe = 1'b1;
		@(posedge test_clk);
		#1;
		test_io_strobe = 1'b0;
		if(dut.magik_lfb_pending) fail("legacy collision did not cancel pending route");
		expect16(dut.magik_lfb_active_seq, 16'd0,
		         "legacy collision clears active sequence");
		transfer_word(16'h4444, response);
		transfer_word(16'h3333, response);
		end_command();
		@(negedge test_clk);
		test_vblank = 1'b0;
		repeat(4) @(posedge test_clk);
		expect16(dut.LFB_BASE[15:0], 16'h4444, "legacy base low wins");
		expect16(dut.LFB_BASE[31:16], 16'h3333, "legacy base high wins");
		expect16(dut.magik_lfb_active_seq, 16'd0, "legacy write clears active sequence");

		$display("COVER LATCH-V5-SYS-TOP actual command counter/strobe path");
		$display("PASS: patched sys_top latch integration");
		$finish;
	end

	initial begin
		#20000;
		fail("global integration simulation timeout");
	end

endmodule

`default_nettype wire
