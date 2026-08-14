// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

`timescale 1ns/1ps
`default_nettype none

module tb_mister_magik_vblank_latch;
	`include "mister_magik_latch_protocol.svh"

	reg clk_sys = 1'b0;
	reg hdmi_vbl = 1'b0;
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
	reg accept_apply = 1'b1;
	reg legacy_write = 1'b0;

	wire response_valid;
	wire [15:0] response_data;
	wire apply;
	wire apply_accepted = apply && accept_apply && !legacy_write;
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
	wire [15:0] reject_count;
	wire [15:0] active_route_epoch;

	reg [31:0] requirement_coverage = 32'd0;
	reg [15:0] status_check_words [0:15];
	integer accepted_apply_count = 0;

	mister_magik_vblank_latch dut (
		.clk_sys(clk_sys),
		.hdmi_vbl(hdmi_vbl),
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
		.apply_accepted(apply_accepted),
		.legacy_write(legacy_write),
		.response_valid(response_valid),
		.response_data(response_data),
		.apply(apply),
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
		.reject_count(reject_count),
		.active_route_epoch(active_route_epoch)
	);

	always #5 clk_sys = ~clk_sys;

	always @(posedge clk_sys) begin
		if(apply_accepted) begin
			accepted_apply_count = accepted_apply_count + 1;
			active_lfb_en <= route_en;
			active_lfb_base <= route_base;
			active_lfb_width <= route_width;
			active_lfb_height <= route_height;
			active_lfb_stride <= route_stride;
		end
	end

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
		input [3:0] index;
		begin
			case(index)
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

	task automatic fail(input [8*128-1:0] message);
		begin
			$display("FAIL: %0s", message);
			$fatal(1);
		end
	endtask

	task automatic expect16(
		input [15:0] actual,
		input [15:0] expected,
		input [8*128-1:0] message
	);
		begin
			if(actual !== expected) begin
				$display("FAIL: %0s: got %04x expected %04x", message, actual, expected);
				$fatal(1);
			end
		end
	endtask

	task automatic expect32(
		input [31:0] actual,
		input [31:0] expected,
		input [8*128-1:0] message
	);
		begin
			if(actual !== expected) begin
				$display("FAIL: %0s: got %08x expected %08x", message, actual, expected);
				$fatal(1);
			end
		end
	endtask

	task automatic expect_true(input condition, input [8*128-1:0] message);
		begin
			if(condition !== 1'b1) fail(message);
		end
	endtask

	task automatic idle_cycles(input integer count);
		integer index;
		begin
			for(index = 0; index < count; index = index + 1) @(posedge clk_sys);
			#1;
		end
	endtask

	task automatic start_command(input [7:0] command, input [15:0] magic);
		begin
			@(negedge clk_sys);
			cmd_id = command;
			cmd_start = 1'b1;
			#1;
			expect_true(response_valid, "recognized command must acknowledge");
			expect16(response_data, magic, "command acknowledgement magic");
			@(posedge clk_sys);
			#1;
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

	task automatic read_word(
		input [7:0] command,
		input [3:0] index,
		output [15:0] value
	);
		begin
			@(negedge clk_sys);
			cmd_id = command;
			word_index = index;
			cmd_data = 1'b1;
			#1;
			expect_true(response_valid, "read word must be valid");
			value = response_data;
			@(posedge clk_sys);
			#1;
			cmd_data = 1'b0;
		end
	endtask

	task automatic send_golden_route;
		integer index;
		begin
			start_command(MAGIK_UIO_SET_FBUF_LATCH, MAGIK_FBUF_LATCH_MAGIC);
			for(index = 0; index < 12; index = index + 1)
				send_word(MAGIK_UIO_SET_FBUF_LATCH, index[3:0],
				          golden_set_word(index[3:0]));
		end
	endtask

	task automatic send_route(
		input [15:0] mode_value,
		input [31:0] base_value,
		input [15:0] width_value,
		input [15:0] height_value,
		input [15:0] hmin_value,
		input [15:0] hmax_value,
		input [15:0] vmin_value,
		input [15:0] vmax_value,
		input [15:0] stride_value,
		input [15:0] seq_value
	);
		reg [15:0] crc;
		begin
			crc = crc_header(MAGIK_UIO_SET_FBUF_LATCH, 16'd11);
			crc = crc_word(crc, mode_value);
			crc = crc_word(crc, base_value[15:0]);
			crc = crc_word(crc, base_value[31:16]);
			crc = crc_word(crc, width_value);
			crc = crc_word(crc, height_value);
			crc = crc_word(crc, hmin_value);
			crc = crc_word(crc, hmax_value);
			crc = crc_word(crc, vmin_value);
			crc = crc_word(crc, vmax_value);
			crc = crc_word(crc, stride_value);
			crc = crc_word(crc, seq_value);
			start_command(MAGIK_UIO_SET_FBUF_LATCH, MAGIK_FBUF_LATCH_MAGIC);
			send_word(MAGIK_UIO_SET_FBUF_LATCH, 4'd0, mode_value);
			send_word(MAGIK_UIO_SET_FBUF_LATCH, 4'd1, base_value[15:0]);
			send_word(MAGIK_UIO_SET_FBUF_LATCH, 4'd2, base_value[31:16]);
			send_word(MAGIK_UIO_SET_FBUF_LATCH, 4'd3, width_value);
			send_word(MAGIK_UIO_SET_FBUF_LATCH, 4'd4, height_value);
			send_word(MAGIK_UIO_SET_FBUF_LATCH, 4'd5, hmin_value);
			send_word(MAGIK_UIO_SET_FBUF_LATCH, 4'd6, hmax_value);
			send_word(MAGIK_UIO_SET_FBUF_LATCH, 4'd7, vmin_value);
			send_word(MAGIK_UIO_SET_FBUF_LATCH, 4'd8, vmax_value);
			send_word(MAGIK_UIO_SET_FBUF_LATCH, 4'd9, stride_value);
			send_word(MAGIK_UIO_SET_FBUF_LATCH, 4'd10, seq_value);
			send_word(MAGIK_UIO_SET_FBUF_LATCH, 4'd11, crc);
		end
	endtask

	task automatic pulse_vblank;
		integer wait_count;
		begin
			@(negedge clk_sys);
			hdmi_vbl = 1'b1;
			for(wait_count = 0; wait_count < 5; wait_count = wait_count + 1)
				@(posedge clk_sys);
			@(negedge clk_sys);
			hdmi_vbl = 1'b0;
			idle_cycles(4);
		end
	endtask

	task automatic reproduce_preserved_no_pending_gap(
		input [15:0] accepted_value,
		input [15:0] active_value
	);
		begin
			$display(
				"REGRESSION: accepted=%0d active=%0d no-pending must be unreachable",
				accepted_value,
				active_value
			);
			if(pending) pulse_vblank();
			send_route(
				MAGIK_GOLDEN_SET_V5_0,
				32'h227e9000,
				16'd960,
				16'd540,
				16'd0,
				16'd1920,
				16'd0,
				16'd1080,
				16'd1920,
				active_value
			);
			expect_true(pending, "regression active setup must be pending");
			pulse_vblank();
			expect16(active_seq, active_value, "regression active setup sequence");
			send_route(
				MAGIK_GOLDEN_SET_V5_0,
				32'h227e9000,
				16'd960,
				16'd540,
				16'd0,
				16'd1920,
				16'd0,
				16'd1080,
				16'd1920,
				accepted_value
			);
			expect16(dut.accepted_seq, accepted_value, "regression accepted sequence");
			expect16(active_seq, active_value, "regression prior active sequence");
			expect_true(pending, "accepted N with active N-1 must retain pending");
			expect16(pending_seq, accepted_value, "regression pending sequence");
			pulse_vblank();
			expect16(active_seq, accepted_value, "regression sequence becomes active");
			expect_true(!pending, "regression pending clears only with activation");
		end
	endtask

	task automatic expect_reject(
		input [15:0] previous_count,
		input [3:0] reason,
		input [8*128-1:0] message
	);
		begin
			expect16(reject_count, previous_count + 1'd1, message);
			expect16(
				{12'd0, dut.last_reject_reason},
				{12'd0, reason},
				"rejection reason"
			);
		end
	endtask

	task automatic corrupt_golden_transaction(input [3:0] corrupt_index);
		integer index;
		reg [15:0] value;
		begin
			start_command(MAGIK_UIO_SET_FBUF_LATCH, MAGIK_FBUF_LATCH_MAGIC);
			for(index = 0; index < 12; index = index + 1) begin
				value = golden_set_word(index[3:0]);
				if(index[3:0] == corrupt_index) value = value ^ 16'h0001;
				send_word(MAGIK_UIO_SET_FBUF_LATCH, index[3:0], value);
			end
		end
	endtask

	task automatic check_status_crc;
		integer status_index;
		reg [15:0] expected_crc;
		reg [15:0] status_word;
		begin
			start_command(MAGIK_UIO_GET_FBUF_LATCH, MAGIK_FBUF_STATUS_MAGIC);
			for(status_index = 0; status_index < 16; status_index = status_index + 1) begin
				read_word(
					MAGIK_UIO_GET_FBUF_LATCH,
					status_index[3:0],
					status_word
				);
				status_check_words[status_index] = status_word;
			end
			expected_crc = crc_header(MAGIK_UIO_GET_FBUF_LATCH, 16'd15);
			for(status_index = 0; status_index < 15; status_index = status_index + 1)
				expected_crc = crc_word(expected_crc, status_check_words[status_index]);
			expect16(status_check_words[15], expected_crc, "randomized status CRC");
		end
	endtask

	initial begin
		integer index;
		integer split;
		reg [15:0] value;
		reg [15:0] crc;
		reg [15:0] reject_before;
		reg [15:0] pending_before;
		reg [15:0] drop_before;
		reg [15:0] flip_before;
		reg [15:0] epoch_before;
		reg [31:0] owned_before;
		reg [31:0] presented_before;
		reg [31:0] repeated_before;
		reg [31:0] ownership_loss_before;
		reg [15:0] snapshot [0:15];
		reg [15:0] random_state;
		reg model_pending;
		reg [15:0] model_pending_seq;
		reg [15:0] model_active_seq;
		reg [15:0] model_post_count;
		reg [15:0] model_flip_count;
		reg [15:0] model_drop_count;
		reg [15:0] model_reject_count;
		reg [15:0] model_epoch;
		reg model_magik_ownership;
		reg [31:0] model_owned_vblank_count;
		reg [31:0] model_presented_vblank_count;
		reg [31:0] model_repeated_vblank_count;
		reg [31:0] model_ownership_loss_count;

		idle_cycles(2);
		expect16(active_seq, 16'd0, "power-up active sequence");
		expect16(pending_seq, 16'd0, "power-up pending sequence");
		expect16(post_count, 16'd0, "power-up post count");
		expect16(flip_count, 16'd0, "power-up flip count");
		expect16(drop_count, 16'd0, "power-up drop count");
		expect16(reject_count, 16'd0, "power-up reject count");
		expect16(active_route_epoch, 16'd0, "power-up route epoch");
		expect32(dut.owned_vblank_count, 32'd0, "power-up owned vblank count");
		expect32(dut.presented_vblank_count, 32'd0, "power-up presented count");
		expect32(dut.repeated_vblank_count, 32'd0, "power-up repeated count");
		expect32(dut.ownership_loss_count, 32'd0, "power-up ownership loss count");

		start_command(MAGIK_UIO_GET_FBUF_LATCH_CAPS, MAGIK_FBUF_CAPS_MAGIC);
		read_word(MAGIK_UIO_GET_FBUF_LATCH_CAPS, 4'd0, value);
		expect16(value, MAGIK_GOLDEN_CAPS_V5_0, "caps version");
		read_word(MAGIK_UIO_GET_FBUF_LATCH_CAPS, 4'd1, value);
		expect16(value, MAGIK_GOLDEN_CAPS_V5_1, "caps flags");
		read_word(MAGIK_UIO_GET_FBUF_LATCH_CAPS, 4'd2, value);
		expect16(value, MAGIK_GOLDEN_CAPS_V5_2, "caps width");
		read_word(MAGIK_UIO_GET_FBUF_LATCH_CAPS, 4'd3, value);
		expect16(value, MAGIK_GOLDEN_CAPS_V5_3, "caps height");
		read_word(MAGIK_UIO_GET_FBUF_LATCH_CAPS, 4'd4, value);
		expect16(value, MAGIK_GOLDEN_CAPS_V5_4, "caps stride");
		read_word(MAGIK_UIO_GET_FBUF_LATCH_CAPS, 4'd5, value);
		expect16(value, MAGIK_GOLDEN_CAPS_V5_CRC, "caps CRC");
		read_word(MAGIK_UIO_GET_FBUF_LATCH_CAPS, 4'd6, value);
		expect16(value, 16'd0, "caps post-close word is zero");
		start_command(
			MAGIK_UIO_GET_FBUF_PRESENTATION_TELEMETRY,
			MAGIK_FBUF_PRESENTATION_TELEMETRY_MAGIC
		);
		for(index = 0; index < 11; index = index + 1)
			read_word(
				MAGIK_UIO_GET_FBUF_PRESENTATION_TELEMETRY,
				index[3:0],
				snapshot[index]
			);
		crc = crc_header(MAGIK_UIO_GET_FBUF_PRESENTATION_TELEMETRY, 16'd10);
		for(index = 0; index < 10; index = index + 1)
			crc = crc_word(crc, snapshot[index]);
		expect16(snapshot[0], 16'd0, "initial telemetry owned low");
		expect16(snapshot[2], 16'd0, "initial telemetry presented low");
		expect16(snapshot[4], 16'd0, "initial telemetry repeated low");
		expect16(snapshot[6], 16'd0, "initial telemetry ownership loss low");
		expect16(snapshot[8], 16'd0, "initial telemetry active sequence");
		expect16(snapshot[9], 16'd0, "initial telemetry flags");
		expect16(snapshot[10], crc, "initial telemetry CRC");
		start_command(
			MAGIK_UIO_GET_FBUF_LATCH_DIAGNOSTICS,
			MAGIK_FBUF_DIAGNOSTICS_MAGIC
		);
		for(index = 0; index < 7; index = index + 1)
			read_word(
				MAGIK_UIO_GET_FBUF_LATCH_DIAGNOSTICS,
				index[3:0],
				snapshot[index]
			);
		crc = crc_header(MAGIK_UIO_GET_FBUF_LATCH_DIAGNOSTICS, 16'd6);
		for(index = 0; index < 6; index = index + 1)
			crc = crc_word(crc, snapshot[index]);
		expect16(snapshot[0], 16'd0, "initial diagnostics reject count");
		expect16(snapshot[1], {12'd0, MAGIK_REJECT_NONE}, "initial diagnostics reason");
		expect16(snapshot[2], 16'hffff, "initial diagnostics expected index");
		expect16(snapshot[3], 16'hffff, "initial diagnostics observed index");
		expect16(snapshot[4], 16'd0, "initial diagnostics observed command");
		expect16(snapshot[5], 16'd0, "initial diagnostics receiver flags");
		expect16(snapshot[6], crc, "initial diagnostics CRC");
		@(negedge clk_sys);
		cmd_id = 8'h5d;
		cmd_start = 1'b1;
		#1;
		expect_true(!response_valid, "unknown command is not acknowledged");
		expect16(response_data, 16'd0, "unknown command response is zero");
		@(posedge clk_sys);
		#1;
		cmd_start = 1'b0;
		requirement_coverage[0] = 1'b1;

		send_golden_route();
		expect_true(pending, "valid CRC must commit pending route");
		expect16(pending_seq, 16'h002b, "golden pending sequence");
		expect16(post_count, 16'd1, "first valid post");
		start_command(
			MAGIK_UIO_GET_FBUF_LATCH_RECEIPT,
			MAGIK_FBUF_RECEIPT_MAGIC
		);
		for(index = 0; index < 11; index = index + 1)
			read_word(
				MAGIK_UIO_GET_FBUF_LATCH_RECEIPT,
				index[3:0],
				snapshot[index]
			);
		crc = crc_header(MAGIK_UIO_GET_FBUF_LATCH_RECEIPT, 16'd10);
		for(index = 0; index < 10; index = index + 1)
			crc = crc_word(crc, snapshot[index]);
		expect16(snapshot[0], 16'd1, "accepted receipt attempted transaction");
		expect16(snapshot[1], 16'h002b, "accepted receipt attempted sequence");
		expect16(snapshot[2], MAGIK_RECEIPT_ACCEPTED, "accepted receipt disposition");
		expect16(snapshot[3], 16'd1, "accepted receipt accepted transaction");
		expect16(snapshot[4], 16'h002b, "accepted receipt accepted sequence");
		expect16(snapshot[5], 16'd1, "accepted receipt pending transaction");
		expect16(snapshot[6], 16'h002b, "accepted receipt pending sequence");
		expect16(snapshot[7], 16'd0, "accepted receipt active transaction");
		expect16(snapshot[8], 16'd0, "accepted receipt active sequence");
		expect16(
			snapshot[9],
			{12'd0, MAGIK_REJECT_NONE},
			"accepted receipt rejection reason"
		);
		expect16(snapshot[10], crc, "accepted receipt CRC");
		if(!route_en || route_flt || (route_fmt != 6'h14) ||
		   (route_base != 32'h227e9000) || (route_width != 12'd960) ||
		   (route_height != 12'd540) || (route_stride != 14'd1920))
			fail("committed golden route bundle mismatch");
		requirement_coverage[1] = 1'b1;

		pulse_vblank();
		expect16(active_seq, 16'h002b, "accepted apply sequence");
		expect16(flip_count, 16'd1, "accepted apply flip count");
		expect16(active_route_epoch, 16'd1, "accepted apply epoch");
		expect_true(!pending, "accepted apply clears pending");
		expect32(accepted_apply_count, 32'd1, "accepted apply pulse count");
		expect32(dut.owned_vblank_count, 32'd1, "first takeover owns one vblank");
		expect32(dut.presented_vblank_count, 32'd1, "first takeover presents once");
		expect32(dut.repeated_vblank_count, 32'd0, "first takeover is not a repeat");
		requirement_coverage[2] = 1'b1;

		start_command(MAGIK_UIO_GET_FBUF_LATCH, MAGIK_FBUF_STATUS_MAGIC);
		for(index = 0; index < 16; index = index + 1)
			read_word(MAGIK_UIO_GET_FBUF_LATCH, index[3:0], snapshot[index]);
		crc = crc_header(MAGIK_UIO_GET_FBUF_LATCH, 16'd15);
		for(index = 0; index < 15; index = index + 1)
			crc = crc_word(crc, snapshot[index]);
		expect16(snapshot[15], crc, "status snapshot CRC");
		expect16(snapshot[0], 16'h002b, "status active sequence");
		expect16(snapshot[2], 16'h0009, "status ownership flags");
		expect16(snapshot[5], 16'h9000, "status active base low");
		expect16(snapshot[6], 16'h227e, "status active base high");
		expect16(snapshot[7], 16'd960, "status active width");
		expect16(snapshot[8], 16'd540, "status active height");
		expect16(snapshot[9], 16'd1920, "status active stride");
		expect16(snapshot[11], 16'd1, "status active epoch");
		expect16(snapshot[12], 16'd1, "status active transaction");
		expect16(snapshot[14], 16'd1, "status accepted transaction");
		requirement_coverage[3] = 1'b1;

		pulse_vblank();
		expect32(dut.owned_vblank_count, 32'd2, "owned idle vblank is counted");
		expect32(dut.presented_vblank_count, 32'd1, "repeat does not present");
		expect32(dut.repeated_vblank_count, 32'd1, "owned idle vblank repeats");
		start_command(
			MAGIK_UIO_GET_FBUF_PRESENTATION_TELEMETRY,
			MAGIK_FBUF_PRESENTATION_TELEMETRY_MAGIC
		);
		for(index = 0; index < 5; index = index + 1)
			read_word(
				MAGIK_UIO_GET_FBUF_PRESENTATION_TELEMETRY,
				index[3:0],
				snapshot[index]
			);
		pulse_vblank();
		for(index = 5; index < 11; index = index + 1)
			read_word(
				MAGIK_UIO_GET_FBUF_PRESENTATION_TELEMETRY,
				index[3:0],
				snapshot[index]
			);
		crc = crc_header(MAGIK_UIO_GET_FBUF_PRESENTATION_TELEMETRY, 16'd10);
		for(index = 0; index < 10; index = index + 1)
			crc = crc_word(crc, snapshot[index]);
		expect16(snapshot[0], 16'd2, "telemetry snapshot owned count is pre-vblank");
		expect16(snapshot[2], 16'd1, "telemetry snapshot presented count is coherent");
		expect16(snapshot[4], 16'd1, "telemetry snapshot repeated count is pre-vblank");
		expect16(snapshot[10], crc, "telemetry snapshot remains CRC coherent");
		expect32(dut.owned_vblank_count, 32'd3, "live owned count advances after snapshot");
		expect32(dut.repeated_vblank_count, 32'd2, "live repeat count advances after snapshot");
		requirement_coverage[12] = 1'b1;

		// A command-start snapshot remains coherent while the live route changes.
		start_command(MAGIK_UIO_GET_FBUF_LATCH, MAGIK_FBUF_STATUS_MAGIC);
		for(index = 0; index < 5; index = index + 1)
			read_word(MAGIK_UIO_GET_FBUF_LATCH, index[3:0], snapshot[index]);
		epoch_before = active_route_epoch;
		legacy_write = 1'b1;
		active_lfb_en = 1'b0;
		active_lfb_base = 32'h11112222;
		active_lfb_width = 12'd320;
		active_lfb_height = 12'd240;
		active_lfb_stride = 14'd640;
		idle_cycles(1);
		legacy_write = 1'b0;
		for(index = 5; index < 16; index = index + 1)
			read_word(MAGIK_UIO_GET_FBUF_LATCH, index[3:0], snapshot[index]);
		crc = crc_header(MAGIK_UIO_GET_FBUF_LATCH, 16'd15);
		for(index = 0; index < 15; index = index + 1)
			crc = crc_word(crc, snapshot[index]);
		expect16(snapshot[15], crc, "status remains one coherent snapshot");
		expect16(snapshot[0], 16'h002b, "snapshot predates legacy takeover");
		expect16(snapshot[5], 16'h9000, "snapshot base predates takeover");
		expect16(active_seq, 16'd0, "legacy takeover clears active sequence");
		expect16(active_route_epoch, epoch_before + 1'd1, "legacy takeover epoch");
		expect32(dut.ownership_loss_count, 32'd1, "owned legacy takeover records loss");
		ownership_loss_before = dut.ownership_loss_count;
		legacy_write = 1'b1;
		idle_cycles(1);
		legacy_write = 1'b0;
		expect32(
			dut.ownership_loss_count,
			ownership_loss_before,
			"legacy write while unowned records no additional loss"
		);
		pulse_vblank();
		expect32(dut.owned_vblank_count, 32'd3, "unowned vblank is excluded");

		// A receipt query finalizes an interrupted SET as one rejected attempt.
		start_command(MAGIK_UIO_SET_FBUF_LATCH, MAGIK_FBUF_LATCH_MAGIC);
		send_word(MAGIK_UIO_SET_FBUF_LATCH, 4'd0, MAGIK_GOLDEN_SET_V5_0);
		reject_before = reject_count;
		start_command(
			MAGIK_UIO_GET_FBUF_LATCH_RECEIPT,
			MAGIK_FBUF_RECEIPT_MAGIC
		);
		for(index = 0; index < 11; index = index + 1)
			read_word(
				MAGIK_UIO_GET_FBUF_LATCH_RECEIPT,
				index[3:0],
				snapshot[index]
			);
		crc = crc_header(MAGIK_UIO_GET_FBUF_LATCH_RECEIPT, 16'd10);
		for(index = 0; index < 10; index = index + 1)
			crc = crc_word(crc, snapshot[index]);
		expect16(reject_count, reject_before + 1'd1, "interrupted SET rejects once");
		expect16(snapshot[0], 16'd2, "rejected receipt attempted transaction");
		expect16(snapshot[1], 16'd0, "interrupted receipt has no posted sequence");
		expect16(snapshot[2], MAGIK_RECEIPT_REJECTED, "rejected receipt disposition");
		expect16(snapshot[3], 16'd0, "rejected receipt accepted transaction");
		expect16(snapshot[4], 16'd0, "rejected receipt accepted sequence");
		expect16(snapshot[5], 16'd0, "rejected receipt pending transaction");
		expect16(snapshot[6], 16'd0, "rejected receipt has no pending sequence");
		expect16(snapshot[7], 16'd0, "rejected receipt active transaction");
		expect16(snapshot[8], 16'd0, "rejected receipt active sequence");
		expect16(
			snapshot[9],
			{12'd0, MAGIK_REJECT_MISSING_WORD},
			"interrupted receipt rejection reason"
		);
		expect16(snapshot[10], crc, "rejected receipt CRC");
		requirement_coverage[4] = 1'b1;

		// Vblank after every SET payload word cannot expose the in-flight bundle.
		for(split = 0; split < 12; split = split + 1) begin
			reject_before = reject_count;
			start_command(MAGIK_UIO_SET_FBUF_LATCH, MAGIK_FBUF_LATCH_MAGIC);
			for(index = 0; index < 12; index = index + 1) begin
				send_word(MAGIK_UIO_SET_FBUF_LATCH, index[3:0],
				          golden_set_word(index[3:0]));
				if(index == split) pulse_vblank();
			end
			expect16(reject_count, reject_before, "vblank sweep valid transaction");
			expect_true(pending || (active_seq == 16'h002b),
			            "vblank sweep exposes only complete route");
			if(pending) pulse_vblank();
		end
		requirement_coverage[5] = 1'b1;

		// Every SET word, including CRC, is protected; pending is unchanged.
		send_route(16'h8014, 32'h20002000, 16'd640, 16'd480,
		           16'd0, 16'd639, 16'd0, 16'd479, 16'd1280, 16'h0040);
		pending_before = pending_seq;
		for(index = 0; index < 12; index = index + 1) begin
			reject_before = reject_count;
			corrupt_golden_transaction(index[3:0]);
			expect_reject(reject_before, MAGIK_REJECT_BAD_CRC,
			              "corrupt SET word rejected once");
			expect16(pending_seq, pending_before, "corruption cannot alter pending");
		end
		requirement_coverage[6] = 1'b1;

		// Framing failures each reject once and leave committed pending untouched.
		reject_before = reject_count;
		start_command(MAGIK_UIO_SET_FBUF_LATCH, MAGIK_FBUF_LATCH_MAGIC);
		send_word(MAGIK_UIO_SET_FBUF_LATCH, 4'd0, MAGIK_GOLDEN_SET_V5_0);
		send_word(MAGIK_UIO_SET_FBUF_LATCH, 4'd11, MAGIK_GOLDEN_SET_V5_CRC);
		expect_reject(reject_before, MAGIK_REJECT_MISSING_WORD, "missing word");
		start_command(
			MAGIK_UIO_GET_FBUF_LATCH_DIAGNOSTICS,
			MAGIK_FBUF_DIAGNOSTICS_MAGIC
		);
		for(index = 0; index < 7; index = index + 1)
			read_word(
				MAGIK_UIO_GET_FBUF_LATCH_DIAGNOSTICS,
				index[3:0],
				snapshot[index]
			);
		crc = crc_header(MAGIK_UIO_GET_FBUF_LATCH_DIAGNOSTICS, 16'd6);
		for(index = 0; index < 6; index = index + 1)
			crc = crc_word(crc, snapshot[index]);
		expect16(snapshot[0], reject_before + 1'd1, "diagnostics reject count");
		expect16(
			snapshot[1],
			{12'd0, MAGIK_REJECT_MISSING_WORD},
			"diagnostics reject reason"
		);
		expect16(snapshot[2], 16'd1, "diagnostics expected word");
		expect16(snapshot[3], 16'd11, "diagnostics observed word");
		expect16(
			snapshot[4],
			{8'd0, MAGIK_UIO_SET_FBUF_LATCH},
			"diagnostics observed command"
		);
		expect16(snapshot[5], 16'd1, "diagnostics pre-reject receiver state");
		expect16(snapshot[6], crc, "diagnostics CRC");

		reject_before = reject_count;
		start_command(MAGIK_UIO_SET_FBUF_LATCH, MAGIK_FBUF_LATCH_MAGIC);
		send_word(MAGIK_UIO_SET_FBUF_LATCH, 4'd0, MAGIK_GOLDEN_SET_V5_0);
		send_word(MAGIK_UIO_SET_FBUF_LATCH, 4'd0, MAGIK_GOLDEN_SET_V5_0);
		expect_reject(reject_before, MAGIK_REJECT_DUPLICATE_WORD, "duplicate word");

		reject_before = reject_count;
		start_command(MAGIK_UIO_SET_FBUF_LATCH, MAGIK_FBUF_LATCH_MAGIC);
		send_word(MAGIK_UIO_SET_FBUF_LATCH, 4'd0, MAGIK_GOLDEN_SET_V5_0);
		send_word(MAGIK_UIO_SET_FBUF_LATCH, 4'd1, MAGIK_GOLDEN_SET_V5_1);
		send_word(MAGIK_UIO_SET_FBUF_LATCH, 4'd0, MAGIK_GOLDEN_SET_V5_0);
		expect_reject(reject_before, MAGIK_REJECT_OUT_OF_ORDER, "reordered word");

		reject_before = reject_count;
		start_command(MAGIK_UIO_SET_FBUF_LATCH, MAGIK_FBUF_LATCH_MAGIC);
		send_word(MAGIK_UIO_SET_FBUF_LATCH, 4'd2, MAGIK_GOLDEN_SET_V5_2);
		expect_reject(reject_before, MAGIK_REJECT_SHIFTED_WORD, "shifted word");

		reject_before = reject_count;
		start_command(MAGIK_UIO_SET_FBUF_LATCH, MAGIK_FBUF_LATCH_MAGIC);
		send_word(MAGIK_UIO_SET_FBUF_LATCH, 4'd0, MAGIK_GOLDEN_SET_V5_0);
		start_command(MAGIK_UIO_SET_FBUF_LATCH, MAGIK_FBUF_LATCH_MAGIC);
		expect_reject(reject_before, MAGIK_REJECT_RESTARTED, "restarted transaction");
		// Close the newly opened replacement before the post-close test.
		start_command(MAGIK_UIO_GET_FBUF_LATCH_CAPS, MAGIK_FBUF_CAPS_MAGIC);
		reject_before = reject_count;
		send_word(MAGIK_UIO_SET_FBUF_LATCH, 4'd0, 16'd0);
		expect16(reject_count, reject_before, "faulted close coalesces extra words");
		if(pending) pulse_vblank();
		send_golden_route();
		reject_before = reject_count;
		send_word(MAGIK_UIO_SET_FBUF_LATCH, 4'd0, 16'd0);
		expect_reject(reject_before, MAGIK_REJECT_POST_CLOSE, "post-close word");
		send_word(MAGIK_UIO_SET_FBUF_LATCH, 4'd1, 16'd0);
		expect16(reject_count, reject_before + 1'd1, "post-close rejection coalesced");
		pulse_vblank();
		requirement_coverage[7] = 1'b1;

		// Semantic validation rejects canonical violations without replacing pending.
		pending_before = pending_seq;
		reject_before = reject_count;
		send_route(16'h8001, 32'h2000, 16'd320, 16'd240,
		           16'd0, 16'd319, 16'd0, 16'd239, 16'd640, 16'h1001);
		expect_reject(reject_before, MAGIK_REJECT_INVALID_MODE, "invalid format");
		reject_before = reject_count;
		send_route(16'h8014, 32'h2001, 16'd320, 16'd240,
		           16'd0, 16'd319, 16'd0, 16'd239, 16'd640, 16'h1002);
		expect_reject(reject_before, MAGIK_REJECT_INVALID_BASE, "unaligned base");
		reject_before = reject_count;
		send_route(16'h8014, 32'h2000, 16'h1140, 16'd240,
		           16'd0, 16'd319, 16'd0, 16'd239, 16'd640, 16'h1008);
		expect_reject(reject_before, MAGIK_REJECT_RESERVED, "reserved geometry bits");
		reject_before = reject_count;
		send_route(16'h8014, 32'h2000, 16'd0, 16'd240,
		           16'd0, 16'd319, 16'd0, 16'd239, 16'd640, 16'h1003);
		expect_reject(reject_before, MAGIK_REJECT_INVALID_GEOMETRY, "zero width");
		reject_before = reject_count;
		send_route(16'h8014, 32'h2000, 16'd320, 16'd240,
		           16'd0, 16'd319, 16'd0, 16'd239, 16'd639, 16'h1004);
		expect_reject(reject_before, MAGIK_REJECT_INVALID_STRIDE, "odd stride");
		reject_before = reject_count;
		send_route(16'h8014, 32'h2000, 16'd320, 16'd240,
		           16'd100, 16'd99, 16'd0, 16'd239, 16'd640, 16'h1005);
		expect_reject(reject_before, MAGIK_REJECT_INVALID_BOUNDS, "reversed bounds");
		reject_before = reject_count;
		send_route(16'h8014, 32'hfffffe00, 16'd320, 16'd2,
		           16'd0, 16'd319, 16'd0, 16'd1, 16'd640, 16'h1006);
		expect_reject(reject_before, MAGIK_REJECT_ADDRESS_WRAP, "address wrap");
		expect16(pending_seq, pending_before, "semantic rejects preserve pending");
		// The next transaction must replace the pipelined wrap result. An
		// exclusive end address exactly at 2^32 is valid.
		send_route(16'h8014, 32'hfffffd80, 16'd320, 16'd1,
		           16'd0, 16'd319, 16'd0, 16'd0, 16'd640, 16'h1007);
		expect16(pending_seq, 16'h1007, "valid route after address wrap commits");
		expect32(route_base, 32'hfffffd80, "boundary route base commits");
		pulse_vblank();
		pending_before = pending_seq;
		reject_before = reject_count;
		send_route(16'h0000, 32'h00000002, 16'd0, 16'd0,
		           16'd0, 16'd0, 16'd0, 16'd0, 16'd0, 16'h1008);
		expect_reject(reject_before, MAGIK_REJECT_INVALID_MODE,
		              "disabled route must be canonical");
		expect16(pending_seq, pending_before, "later semantic reject preserves pending");
		requirement_coverage[8] = 1'b1;

		// A SET completed on the old pending's apply edge is still rejected.
		if(!pending) send_golden_route();
		drop_before = drop_count;
		flip_before = flip_count;
		reject_before = reject_count;
		start_command(MAGIK_UIO_SET_FBUF_LATCH, MAGIK_FBUF_LATCH_MAGIC);
		for(index = 0; index < 11; index = index + 1)
			send_word(MAGIK_UIO_SET_FBUF_LATCH, index[3:0],
			          golden_set_word(index[3:0]));
		@(negedge clk_sys);
		hdmi_vbl = 1'b1;
		while(!apply) @(negedge clk_sys);
		cmd_id = MAGIK_UIO_SET_FBUF_LATCH;
		word_index = 4'd11;
		data_in = MAGIK_GOLDEN_SET_V5_CRC;
		cmd_data = 1'b1;
		@(posedge clk_sys);
		#1;
		cmd_data = 1'b0;
		expect16(flip_count, flip_before + 1'd1, "old pending applied on commit edge");
		expect_true(!pending, "old pending clears on its accepted apply edge");
		expect16(drop_count, drop_before + 1'd1, "busy SET is counted without replacement");
		expect_reject(
			reject_before,
			MAGIK_REJECT_PENDING_BUSY,
			"simultaneous apply/SET rejects the new transaction"
		);
		@(negedge clk_sys);
		hdmi_vbl = 1'b0;
		idle_cycles(4);
		requirement_coverage[9] = 1'b1;

		// Legacy writes win same-edge arbitration and cancel MagiK's pending route.
		send_golden_route();
		epoch_before = active_route_epoch;
		flip_before = flip_count;
		owned_before = dut.owned_vblank_count;
		presented_before = dut.presented_vblank_count;
		repeated_before = dut.repeated_vblank_count;
		ownership_loss_before = dut.ownership_loss_count;
		@(negedge clk_sys);
		hdmi_vbl = 1'b1;
		while(!apply) @(negedge clk_sys);
		legacy_write = 1'b1;
		active_lfb_en = 1'b0;
		active_lfb_base = 32'h33334444;
		active_lfb_width = 12'd640;
		active_lfb_height = 12'd480;
		active_lfb_stride = 14'd1280;
		@(posedge clk_sys);
		#1;
		legacy_write = 1'b0;
		expect16(flip_count, flip_before, "unaccepted apply cannot flip");
		expect_true(!pending, "legacy winner cancels MagiK pending state");
		expect16(active_seq, 16'd0, "legacy winner clears ownership sequence");
		expect16(active_route_epoch, epoch_before + 1'd1, "legacy winner advances epoch");
		expect32(dut.owned_vblank_count, owned_before, "legacy winner excludes collision vblank");
		expect32(
			dut.presented_vblank_count,
			presented_before,
			"legacy winner excludes collision presentation"
		);
		expect32(
			dut.repeated_vblank_count,
			repeated_before,
			"legacy winner excludes collision repeat"
		);
		expect32(
			dut.ownership_loss_count,
			ownership_loss_before + 1'd1,
			"legacy winner records one ownership loss"
		);
		@(negedge clk_sys);
		hdmi_vbl = 1'b0;
		idle_cycles(4);
		send_golden_route();
		pulse_vblank();
		expect_true(!pending, "new route applies after ownership returns");
		expect16(active_seq, 16'h002b, "MagiK ownership returns after accepted apply");
		requirement_coverage[10] = 1'b1;

		reproduce_preserved_no_pending_gap(16'd1213, 16'd1212);
		reproduce_preserved_no_pending_gap(16'd962, 16'd961);

		// Counter and sequence/epoch wrap remain explicit.
		@(negedge clk_sys);
		dut.post_count = 16'hffff;
		dut.flip_count = 16'hffff;
		dut.drop_count = 16'hffff;
		dut.reject_count = 16'hffff;
		dut.active_route_epoch = 16'hffff;
		dut.owned_vblank_count = 32'hffff_ffff;
		dut.presented_vblank_count = 32'hffff_fffe;
		dut.repeated_vblank_count = 32'd1;
		dut.ownership_loss_count = 32'hffff_ffff;
		send_route(16'h0000, 32'd0, 16'd0, 16'd0,
		           16'd0, 16'd0, 16'd0, 16'd0, 16'd0, 16'hffff);
		expect16(post_count, 16'd0, "post counter wrap");
		send_route(16'h0000, 32'd0, 16'd0, 16'd0,
		           16'd0, 16'd0, 16'd0, 16'd0, 16'd0, 16'd0);
		expect16(drop_count, 16'd0, "drop counter wrap");
		pulse_vblank();
		expect16(flip_count, 16'd0, "flip counter wrap");
		expect32(dut.owned_vblank_count, 32'd0, "owned vblank counter wrap");
		expect32(dut.presented_vblank_count, 32'hffff_ffff, "presented counter advances at wrap");
		expect16(active_seq, 16'hffff, "first wrap-boundary sequence applies");
		expect16(active_route_epoch, 16'd0, "route epoch wrap");
		send_route(16'h0000, 32'd0, 16'd0, 16'd0,
		           16'd0, 16'd0, 16'd0, 16'd0, 16'd0, 16'd0);
		pulse_vblank();
		expect16(active_seq, 16'd0, "sequence wrap");
		expect32(dut.owned_vblank_count, 32'd1, "owned counter advances after wrap");
		expect32(dut.presented_vblank_count, 32'd0, "presented counter wraps");
		legacy_write = 1'b1;
		idle_cycles(1);
		legacy_write = 1'b0;
		expect32(dut.ownership_loss_count, 32'd0, "ownership loss counter wrap");
		reject_before = reject_count;
		corrupt_golden_transaction(4'd11);
		expect16(reject_count, reject_before + 1'd1, "reject counter advances after wrap");
		requirement_coverage[11] = 1'b1;

		// Fixed-seed transaction/vblank/legacy interleavings against a counter
		// and sequence reference model.
		random_state = 16'h1d0f;
		model_pending = pending;
		model_pending_seq = pending_seq;
		model_active_seq = active_seq;
		model_post_count = post_count;
		model_flip_count = flip_count;
		model_drop_count = drop_count;
		model_reject_count = reject_count;
		model_epoch = active_route_epoch;
		model_magik_ownership = dut.magik_ownership;
		model_owned_vblank_count = dut.owned_vblank_count;
		model_presented_vblank_count = dut.presented_vblank_count;
		model_repeated_vblank_count = dut.repeated_vblank_count;
		model_ownership_loss_count = dut.ownership_loss_count;
		for(index = 0; index < 96; index = index + 1) begin
			random_state = {random_state[14:0],
			                random_state[15] ^ random_state[13] ^
			                random_state[12] ^ random_state[10]};
			case(random_state[2:0])
				3'd0: begin
					send_golden_route();
					if(model_pending) begin
						model_drop_count = model_drop_count + 1'd1;
						model_reject_count = model_reject_count + 1'd1;
					end
					else begin
						model_pending = 1'b1;
						model_pending_seq = 16'h002b;
						model_post_count = model_post_count + 1'd1;
					end
				end
				3'd1: begin
					send_route(16'd0, 32'd0, 16'd0, 16'd0,
					           16'd0, 16'd0, 16'd0, 16'd0, 16'd0,
					           random_state);
					if(model_pending) begin
						model_drop_count = model_drop_count + 1'd1;
						model_reject_count = model_reject_count + 1'd1;
					end
					else begin
						model_pending = 1'b1;
						model_pending_seq = random_state;
						model_post_count = model_post_count + 1'd1;
					end
				end
				3'd2: begin
					corrupt_golden_transaction({1'b0, random_state[2:0]});
					model_reject_count = model_reject_count + 1'd1;
				end
				3'd3: begin
					pulse_vblank();
					if(model_pending) begin
						model_owned_vblank_count = model_owned_vblank_count + 1'd1;
						model_presented_vblank_count = model_presented_vblank_count + 1'd1;
						model_magik_ownership = 1'b1;
						model_active_seq = model_pending_seq;
						model_pending = 1'b0;
						model_pending_seq = 16'd0;
						model_flip_count = model_flip_count + 1'd1;
						model_epoch = model_epoch + 1'd1;
					end
					else if(model_magik_ownership) begin
						model_owned_vblank_count = model_owned_vblank_count + 1'd1;
						model_repeated_vblank_count = model_repeated_vblank_count + 1'd1;
					end
				end
				3'd4: begin
					if(model_magik_ownership)
						model_ownership_loss_count = model_ownership_loss_count + 1'd1;
					legacy_write = 1'b1;
					active_lfb_en = 1'b0;
					active_lfb_base = {16'h4000, random_state};
					active_lfb_width = 12'd320;
					active_lfb_height = 12'd240;
					active_lfb_stride = 14'd640;
					idle_cycles(1);
					legacy_write = 1'b0;
					model_active_seq = 16'd0;
					model_magik_ownership = 1'b0;
					model_pending = 1'b0;
					model_pending_seq = 16'd0;
					model_epoch = model_epoch + 1'd1;
				end
				default: check_status_crc();
			endcase
			expect16({15'd0, pending}, {15'd0, model_pending},
			         "randomized pending model");
			expect16(pending_seq, model_pending_seq, "randomized pending sequence");
			expect16(active_seq, model_active_seq, "randomized active sequence");
			expect16(post_count, model_post_count, "randomized post count");
			expect16(flip_count, model_flip_count, "randomized flip count");
			expect16(drop_count, model_drop_count, "randomized drop count");
			expect16(reject_count, model_reject_count, "randomized reject count");
			expect16(active_route_epoch, model_epoch, "randomized route epoch");
			expect32(dut.owned_vblank_count, model_owned_vblank_count,
			         "randomized owned vblank count");
			expect32(dut.presented_vblank_count, model_presented_vblank_count,
			         "randomized presented count");
			expect32(dut.repeated_vblank_count, model_repeated_vblank_count,
			         "randomized repeated count");
			expect32(dut.ownership_loss_count, model_ownership_loss_count,
			         "randomized ownership loss count");
		end
		$display("COVER LATCH-V5-RANDOM fixed-seed reference-model interleavings");

		if(requirement_coverage[12:0] !== 13'h1fff)
			fail("not all protocol-v5 RTL requirement coverpoints hit");
		$display("COVER LATCH-V5-001..LATCH-V5-013 all RTL requirements hit");
		$display("PASS: atomic protocol-v5 latch, coherent status, and ownership arbitration");
		$finish;
	end

	initial begin
		#200000;
		fail("global simulation timeout");
	end

endmodule

`default_nettype wire
