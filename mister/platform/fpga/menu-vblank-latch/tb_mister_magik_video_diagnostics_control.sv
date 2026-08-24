// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

`timescale 1ns/1ps

module tb_mister_magik_video_diagnostics_control;
	`include "mister_magik_video_diagnostics_protocol.svh"

	reg clk_hdmi = 1'b0;
	reg clk_sys = 1'b0;
	reg reset_active = 1'b1;
	reg raw_ce = 1'b0;
	reg [23:0] raw_rgb = 24'd0;
	reg raw_de = 1'b0;
	reg raw_hs = 1'b0;
	reg raw_vs = 1'b0;
	reg io_uio = 1'b0;
	reg io_strobe = 1'b0;
	reg [15:0] io_din = 16'd0;
	wire response_valid;
	wire [15:0] response_data;
	reg [15:0] words [0:5];
	reg [15:0] immutable_words [0:5];
	reg [15:0] first_signature;
	reg [15:0] second_signature;
	integer index;

	mister_magik_raw_scaler_ordered_frame dut (
		.clk_hdmi(clk_hdmi),
		.clk_sys(clk_sys),
		.reset_active(reset_active),
		.raw_ce(raw_ce),
		.raw_rgb(raw_rgb),
		.raw_de(raw_de),
		.raw_hs(raw_hs),
		.raw_vs(raw_vs),
		.io_uio(io_uio),
		.io_strobe(io_strobe),
		.io_din(io_din),
		.response_valid(response_valid),
		.response_data(response_data)
	);

	always #4 clk_hdmi = ~clk_hdmi;
	always #5 clk_sys = ~clk_sys;

	task automatic fail(input [8*112-1:0] message);
		begin
			$display("FAIL: %0s", message);
			$fatal(1);
		end
	endtask

	function automatic [15:0] crc16_byte;
		input [15:0] current;
		input [7:0] value;
		integer bit_index;
		reg [15:0] next;
		begin
			next = current ^ {value, 8'h00};
			for(bit_index = 0; bit_index < 8; bit_index = bit_index + 1)
				next = next[15] ? ((next << 1) ^ 16'h1021) : (next << 1);
			crc16_byte = next;
		end
	endfunction

	function automatic [15:0] crc16_word;
		input [15:0] current;
		input [15:0] value;
		begin
			crc16_word = crc16_byte(crc16_byte(current, value[15:8]), value[7:0]);
		end
	endfunction

	function automatic [15:0] response_crc;
		input integer last_payload_word;
		integer word_index;
		reg [15:0] value;
		begin
			value = 16'hffff;
			value = crc16_word(value, {8'd0, MAGIK_UIO_GET_RAW_SCALER_STATE});
			value = crc16_word(value, MAGIK_RAW_SCALER_STATE_SCHEMA);
			value = crc16_word(value, MAGIK_RAW_SCALER_STATE_WORDS - 1'd1);
			for(word_index = 0; word_index <= last_payload_word; word_index = word_index + 1)
				value = crc16_word(value, words[word_index]);
			response_crc = value;
		end
	endfunction

	// Independent executable model of the one-step ordered signature.
	function automatic [15:0] golden_update;
		input [15:0] current;
		input [31:0] token;
		reg [15:0] mixed;
		begin
			mixed = current ^ token[15:0] ^ token[31:16];
			golden_update = (mixed >> 1) ^
				(mixed[0] ? 16'ha001 : 16'd0);
		end
	endfunction

	function automatic [15:0] golden_rgb565(input [23:0] rgb);
		golden_rgb565 = {rgb[23:19], rgb[15:10], rgb[7:3]};
	endfunction

	function automatic [31:0] golden_pixel_token(
		input [7:0] header,
		input [23:0] rgb
	);
		golden_pixel_token = {header, golden_rgb565(rgb), 8'd0};
	endfunction

	function automatic [15:0] golden_frame_signature(input [7:0] seed, input swapped);
		reg [15:0] next;
		reg [7:0] first_line;
		reg [7:0] second_line;
		begin
			first_line = swapped ? seed + 8'd32 : seed;
			second_line = swapped ? seed : seed + 8'd32;
			next = 16'h56da;
			next = golden_update(next,
				golden_pixel_token(8'hc1, {first_line, 8'h10, 8'h20}));
			next = golden_update(next,
				golden_pixel_token(8'h41, {first_line + 1'd1, 8'h11, 8'h21}));
			next = golden_update(next, {8'ha0, 24'd0});
			next = golden_update(next,
				golden_pixel_token(8'h81, {second_line, 8'h12, 8'h22}));
			next = golden_update(next,
				golden_pixel_token(8'h01, {second_line + 1'd1, 8'h13, 8'h23}));
			golden_frame_signature = golden_update(next, {8'he0, 24'd0});
		end
	endfunction

	task automatic raw_sample(
		input sample_ce,
		input sample_vs,
		input sample_hs,
		input sample_de,
		input [23:0] sample_rgb
	);
		begin
			@(negedge clk_hdmi);
			raw_ce = sample_ce;
			raw_vs = sample_vs;
			raw_hs = sample_hs;
			raw_de = sample_de;
			raw_rgb = sample_rgb;
			@(posedge clk_hdmi);
		end
	endtask

	// Start and complete one two-line, four-pixel frame. The next call's first
	// VS edge completes the preceding frame, matching production framing.
	task automatic drive_frame(input [7:0] seed, input empty, input swapped);
		reg [7:0] first_line;
		reg [7:0] second_line;
		begin
			first_line = swapped ? seed + 8'd32 : seed;
			second_line = swapped ? seed : seed + 8'd32;
			raw_sample(1'b1, 1'b1, 1'b0, 1'b0, 24'd0);
			raw_sample(1'b1, 1'b0, 1'b0, 1'b0, 24'd0);
			if(!empty) begin
				raw_sample(1'b1, 1'b0, 1'b1, 1'b1, {first_line, 8'h10, 8'h20});
				raw_sample(1'b1, 1'b0, 1'b1, 1'b1, {first_line + 1'd1, 8'h11, 8'h21});
				raw_sample(1'b1, 1'b0, 1'b0, 1'b0, 24'd0);
				raw_sample(1'b1, 1'b0, 1'b0, 1'b1, {second_line, 8'h12, 8'h22});
				raw_sample(1'b1, 1'b0, 1'b0, 1'b1, {second_line + 1'd1, 8'h13, 8'h23});
				raw_sample(1'b1, 1'b0, 1'b1, 1'b0, 24'd0);
			end
		end
	endtask

	task automatic complete_frame;
		begin
			raw_sample(1'b1, 1'b1, 1'b0, 1'b0, 24'd0);
			raw_sample(1'b1, 1'b0, 1'b0, 1'b0, 24'd0);
			repeat(8) @(posedge clk_sys);
		end
	endtask

	task automatic command_start(input [7:0] command);
		begin
			@(negedge clk_sys);
			io_uio = 1'b1;
			io_strobe = 1'b1;
			io_din = {8'd0, command};
			#1;
			if(command == MAGIK_UIO_GET_RAW_SCALER_STATE) begin
				if(!response_valid || response_data != MAGIK_RAW_SCALER_STATE_MAGIC)
					fail("0x67 magic response mismatch");
			end
			else if(response_valid)
				fail("unsupported diagnostic command responded");
			@(posedge clk_sys);
			#1 io_strobe = 1'b0;
		end
	endtask

	task automatic read_word(output [15:0] value);
		begin
			@(negedge clk_sys);
			io_strobe = 1'b1;
			io_din = 16'd0;
			#1;
			if(!response_valid)
				fail("0x67 response ended before fixed word count");
			value = response_data;
			@(posedge clk_sys);
			#1 io_strobe = 1'b0;
		end
	endtask

	task automatic command_end;
		begin
			@(negedge clk_sys);
			io_uio = 1'b0;
			io_strobe = 1'b0;
			@(posedge clk_sys);
		end
	endtask

	task automatic read_record;
		begin
			command_start(MAGIK_UIO_GET_RAW_SCALER_STATE);
			for(index = 0; index < 5; index = index + 1)
				read_word(words[index]);
			command_end();
			if(words[0] != MAGIK_RAW_SCALER_STATE_SCHEMA)
				fail("schema mismatch");
			if(words[4] != response_crc(3))
				fail("response CRC mismatch");
		end
	endtask

	initial begin
		repeat(4) @(posedge clk_sys);
		reset_active = 1'b0;
		repeat(3) @(posedge clk_hdmi);

		for(index = 8'h60; index <= 8'h66; index = index + 1) begin
			command_start(index[7:0]);
			command_end();
		end

		drive_frame(8'h11, 1'b0, 1'b0);
		// Completion first commits the stable bundle and toggles the source.
		// The dedicated launch register must remain at the old generation for
		// that edge, then advance exactly one HDMI edge later.
		raw_sample(1'b1, 1'b1, 1'b0, 1'b0, 24'd0);
		raw_sample(1'b1, 1'b0, 1'b0, 1'b0, 24'd0);
		#1;
		if(dut.generation_launch == dut.source_generation)
			fail("generation launch did not delay the source toggle");
		if(dut.published_signature != golden_frame_signature(8'h11, 1'b0))
			fail("signature bundle was not stable before launch");
		@(posedge clk_hdmi);
		#1;
		if(dut.generation_launch != dut.source_generation)
			fail("generation launch did not forward on the next HDMI edge");
		if(dut.published_signature != golden_frame_signature(8'h11, 1'b0))
			fail("signature bundle changed during generation launch");
		repeat(8) @(posedge clk_sys);
		read_record();
		if(words[1] != MAGIK_RAW_SCALER_STATE_FLAG_FRAME_VALID || words[2] != 16'd1)
			fail("first completed-frame flags or sequence mismatch");
		first_signature = words[3];
		if(first_signature != golden_frame_signature(8'h11, 1'b0))
			fail("ordered signature model mismatch");

		// An empty frame cannot replace or advance completed nonempty evidence.
		drive_frame(8'h00, 1'b1, 1'b0);
		complete_frame();
		read_record();
		if(words[2] != 16'd1 || words[3] != first_signature)
			fail("empty frame changed retained evidence");

		// Repeating the exact frame advances sequence but preserves signature.
		drive_frame(8'h11, 1'b0, 1'b0);
		complete_frame();
		read_record();
		if(words[2] != 16'd2 || words[3] != first_signature)
			fail("static ordered frame did not retain its signature");

		// Swapping complete lines must change the order-sensitive signature.
		drive_frame(8'h11, 1'b0, 1'b1);
		complete_frame();
		read_record();
		second_signature = words[3];
		if(second_signature != golden_frame_signature(8'h11, 1'b1) ||
		   second_signature == first_signature)
			fail("line-order change was not detected");

		// A partial read cannot leak its word index into the next transaction.
		command_start(MAGIK_UIO_GET_RAW_SCALER_STATE);
		read_word(words[0]);
		read_word(words[1]);
		command_end();
		read_record();

		// Snapshot remains immutable even if another frame completes mid-read.
		command_start(MAGIK_UIO_GET_RAW_SCALER_STATE);
		for(index = 0; index < 3; index = index + 1)
			read_word(immutable_words[index]);
		drive_frame(8'h44, 1'b0, 1'b0);
		complete_frame();
		for(index = 3; index < 5; index = index + 1)
			read_word(immutable_words[index]);
		command_end();
		for(index = 0; index < 5; index = index + 1)
			words[index] = immutable_words[index];
		if(words[4] != response_crc(3))
			fail("immutable mid-read snapshot CRC mismatch");

		// Sequence wrap remains observable.
		drive_frame(8'h55, 1'b0, 1'b0);
		dut.snapshot_state[15:0] = 16'hffff;
		complete_frame();
		read_record();
		if(words[1] != MAGIK_RAW_SCALER_STATE_FLAG_FRAME_VALID ||
		   words[2] != 16'h0000)
			fail("completed-frame valid state or sequence did not survive wrap");

		// Reset during an active frame clears the coherent source and snapshot.
		drive_frame(8'h99, 1'b0, 1'b0);
		raw_sample(1'b1, 1'b0, 1'b0, 1'b1, 24'habcdef);
		reset_active = 1'b1;
		repeat(2) @(posedge clk_hdmi);
		repeat(2) @(posedge clk_sys);
		reset_active = 1'b0;
		repeat(6) @(posedge clk_sys);
		read_record();
		if(dut.generation_launch != 1'b0 || words[1] != 16'd0 || words[2] != 16'd0 ||
		   words[3] != 16'd0)
			fail("reset did not clear observer snapshot coherently");

		$display("PASS: raw scaler ordered-signature observer framing and state");
		$finish;
	end
endmodule
