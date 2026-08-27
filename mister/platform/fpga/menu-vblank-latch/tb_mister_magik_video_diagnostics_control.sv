// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

`timescale 1ns/1ps

module tb_mister_magik_video_diagnostics_control;
	`include "mister_magik_video_diagnostics_protocol.svh"

	localparam [15:0] FETCH_STATE_SCHEMA = MAGIK_RAW_SCALER_STATE_SCHEMA;
	localparam [15:0] FETCH_FLAG_CAPTURE_VALID =
		MAGIK_RAW_SCALER_STATE_FLAG_CAPTURE_VALID;
	localparam [15:0] FETCH_FLAG_FIFO_OVERFLOW =
		MAGIK_RAW_SCALER_STATE_FLAG_FIFO_OVERFLOW;
	localparam [15:0] FETCH_FLAG_UNEXPECTED_RETURN =
		MAGIK_RAW_SCALER_STATE_FLAG_UNEXPECTED_RETURN;
	localparam [15:0] FETCH_FLAG_BAD_BURSTCOUNT =
		MAGIK_RAW_SCALER_STATE_FLAG_BAD_BURSTCOUNT;
	localparam [15:0] FETCH_FLAG_BAD_RETURN_PHASE =
		MAGIK_RAW_SCALER_STATE_FLAG_BAD_RETURN_PHASE;
	localparam [15:0] FETCH_FLAG_EPOCH_OVERLAP =
		MAGIK_RAW_SCALER_STATE_FLAG_EPOCH_OVERLAP;
	localparam [15:0] FETCH_FLAG_COUNTER_OVERFLOW =
		MAGIK_RAW_SCALER_STATE_FLAG_COUNTER_OVERFLOW;

	reg clk_100m = 1'b0;
	reg clk_sys = 1'b0;
	reg reset_active = 1'b1;
	reg [27:0] vbuf_address = 28'd0;
	reg [7:0] vbuf_burstcount = 8'd128;
	reg vbuf_waitrequest = 1'b0;
	reg [127:0] vbuf_readdata = 128'd0;
	reg vbuf_readdatavalid = 1'b0;
	reg vbuf_read = 1'b0;
	reg io_uio = 1'b0;
	reg io_strobe = 1'b0;
	reg [15:0] io_din = 16'd0;
	wire response_valid;
	wire [15:0] response_data;
	reg [15:0] words [0:4];
	reg [15:0] immutable_words [0:4];
	reg [15:0] stable_signature;
	reg [15:0] changed_signature;
	integer index;

	mister_magik_scaler_fetch_ordered_frame dut (
		.clk_100m(clk_100m),
		.clk_sys(clk_sys),
		.reset_active(reset_active),
		.vbuf_address(vbuf_address),
		.vbuf_burstcount(vbuf_burstcount),
		.vbuf_waitrequest(vbuf_waitrequest),
		.vbuf_readdata(vbuf_readdata),
		.vbuf_readdatavalid(vbuf_readdatavalid),
		.vbuf_read(vbuf_read),
		.io_uio(io_uio),
		.io_strobe(io_strobe),
		.io_din(io_din),
		.response_valid(response_valid),
		.response_data(response_data)
	);

	always #5 clk_100m = ~clk_100m;
	always #7 clk_sys = ~clk_sys;

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
		reg [15:0] next_value;
		begin
			next_value = current ^ {value, 8'h00};
			for(bit_index = 0; bit_index < 8; bit_index = bit_index + 1)
				next_value = next_value[15] ?
					((next_value << 1) ^ 16'h1021) : (next_value << 1);
			crc16_byte = next_value;
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
			value = crc16_word(value, FETCH_STATE_SCHEMA);
			value = crc16_word(value, MAGIK_RAW_SCALER_STATE_WORDS - 1'd1);
			for(word_index = 0; word_index <= last_payload_word;
				word_index = word_index + 1)
				value = crc16_word(value, words[word_index]);
			response_crc = value;
		end
	endfunction

	function automatic [15:0] golden_update;
		input [15:0] current;
		input [15:0] token;
		reg [15:0] mixed;
		begin
			mixed = current ^ token;
			golden_update = {mixed[14:0], mixed[15] ^ mixed[0]};
		end
	endfunction

	function automatic [127:0] beat_data;
		input [7:0] seed;
		input [7:0] beat;
		integer lane;
		reg [127:0] value;
		begin
			value = 128'd0;
			for(lane = 0; lane < 8; lane = lane + 1)
				value[lane * 16 +: 16] = {seed + lane[7:0], beat};
			beat_data = value;
		end
	endfunction

	function automatic [127:0] beat_data_lane_swapped;
		input [7:0] seed;
		input [7:0] beat;
		reg [127:0] value;
		begin
			value = beat_data(seed, beat);
			value[15:0] = {seed + 1'd1, beat};
			value[31:16] = {seed, beat};
			beat_data_lane_swapped = value;
		end
	endfunction

	function automatic [15:0] golden_fold_data;
		input [127:0] data;
		begin
			golden_fold_data =
				data[15:0] ^
				{data[30:16], data[31]} ^
				{data[44:32], data[47:45]} ^
				{data[58:48], data[63:59]} ^
				{data[72:64], data[79:73]} ^
				{data[86:80], data[95:87]} ^
				{data[100:96], data[111:101]} ^
				{data[114:112], data[127:115]};
		end
	endfunction

	function automatic [3:0] golden_fold_address;
		input [27:0] address;
		begin
			golden_fold_address = address[10:7] ^ address[18:15];
		end
	endfunction

	function automatic [15:0] golden_burst;
		input [15:0] current;
		input [27:0] address;
		input [7:0] seed;
		integer beat;
		reg [15:0] next_value;
		reg [15:0] token;
		begin
			next_value = current;
			for(beat = 0; beat < 128; beat = beat + 1) begin
				token = golden_fold_data(beat_data(seed, beat[7:0])) ^ 16'h5a02;
				if(beat == 0)
					next_value = golden_update(
						next_value, {12'ha5a, golden_fold_address(address)});
				next_value = golden_update(next_value, token);
			end
			golden_burst = next_value;
		end
	endfunction

	function automatic [15:0] golden_burst_lane_swapped;
		input [15:0] current;
		input [27:0] address;
		input [7:0] seed;
		integer beat;
		reg [15:0] next_value;
		reg [15:0] token;
		begin
			next_value = current;
			for(beat = 0; beat < 128; beat = beat + 1) begin
				token = golden_fold_data(
					beat_data_lane_swapped(seed, beat[7:0])) ^ 16'h5a02;
				if(beat == 0)
					next_value = golden_update(
						next_value, {12'ha5a, golden_fold_address(address)});
				next_value = golden_update(next_value, token);
			end
			golden_burst_lane_swapped = next_value;
		end
	endfunction

	task automatic pulse_request;
		input [27:0] address;
		input [7:0] burstcount;
		input stalled_first;
		begin
			@(negedge clk_100m);
			vbuf_address = address;
			vbuf_burstcount = burstcount;
			vbuf_read = 1'b1;
			vbuf_waitrequest = stalled_first;
			@(posedge clk_100m);
			if(stalled_first) begin
				@(negedge clk_100m);
				vbuf_waitrequest = 1'b0;
				@(posedge clk_100m);
			end
			@(negedge clk_100m);
			vbuf_read = 1'b0;
		end
	endtask

	task automatic drive_return_burst;
		input [7:0] seed;
		integer beat;
		begin
			for(beat = 0; beat < 128; beat = beat + 1) begin
				@(negedge clk_100m);
				vbuf_readdatavalid = 1'b1;
				vbuf_readdata = beat_data(seed, beat[7:0]);
				@(posedge clk_100m);
			end
			@(negedge clk_100m);
			vbuf_readdatavalid = 1'b0;
			vbuf_readdata = 128'd0;
		end
	endtask

	task automatic drive_return_burst_lane_swapped;
		input [7:0] seed;
		integer beat;
		begin
			for(beat = 0; beat < 128; beat = beat + 1) begin
				@(negedge clk_100m);
				vbuf_readdatavalid = 1'b1;
				vbuf_readdata = beat_data_lane_swapped(seed, beat[7:0]);
				@(posedge clk_100m);
			end
			@(negedge clk_100m);
			vbuf_readdatavalid = 1'b0;
			vbuf_readdata = 128'd0;
		end
	endtask

	task automatic drive_return_burst_with_final_accept;
		input [7:0] seed;
		input [27:0] accepted_address;
		integer beat;
		begin
			for(beat = 0; beat < 128; beat = beat + 1) begin
				@(negedge clk_100m);
				vbuf_readdatavalid = 1'b1;
				vbuf_readdata = beat_data(seed, beat[7:0]);
				if(beat == 127) begin
					vbuf_address = accepted_address;
					vbuf_burstcount = 8'd128;
					vbuf_waitrequest = 1'b0;
					vbuf_read = 1'b1;
				end
				@(posedge clk_100m);
			end
			@(negedge clk_100m);
			vbuf_readdatavalid = 1'b0;
			vbuf_readdata = 128'd0;
			vbuf_read = 1'b0;
		end
	endtask

	task automatic wait_capture;
		begin
			repeat(64) @(posedge clk_sys);
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
			if(words[0] != FETCH_STATE_SCHEMA)
				fail("schema mismatch");
			if(words[4] != response_crc(3))
				fail("response CRC mismatch");
		end
	endtask

	task automatic apply_reset;
		begin
			reset_active = 1'b1;
			repeat(3) @(posedge clk_100m);
			repeat(2) @(posedge clk_sys);
			reset_active = 1'b0;
			repeat(3) @(posedge clk_100m);
			repeat(64) @(posedge clk_sys);
		end
	endtask

	initial begin
		reg [15:0] golden;
		reg prior_generation;
		apply_reset();

		for(index = 8'h60; index <= 8'h66; index = index + 1) begin
			command_start(index[7:0]);
			command_end();
		end

		read_record();
		if(words[1] != 16'd0 || words[2] != 16'd0 || words[3] != 16'd0)
			fail("pre-alignment record was not empty");

		// A full two-entry FIFO may dequeue its final beat and enqueue the next
		// accepted request on the same edge without an overflow or reordering.
		pulse_request(28'h0001000, 8'd128, 1'b0);
		pulse_request(28'h0001080, 8'd128, 1'b0);
		drive_return_burst_with_final_accept(8'h01, 28'h0001100);
		drive_return_burst(8'h02);
		drive_return_burst(8'h03);
		wait_capture();
		read_record();
		if(words[1] != 16'd0 || words[2] != 16'd0 || words[3] != 16'd0)
			fail("simultaneous full-FIFO dequeue/enqueue corrupted pre-alignment state");
		apply_reset();

		// One ignored pre-alignment transaction establishes a high address.
		pulse_request(28'h0001800, 8'd128, 1'b1);
		drive_return_burst(8'h70);

		// First wrap arms. Two accepted requests are deliberately outstanding;
		// the following wrap is queued behind the second request.
		pulse_request(28'h0000800, 8'd128, 1'b0);
		pulse_request(28'h0000880, 8'd128, 1'b0);
		drive_return_burst(8'h11);
		pulse_request(28'h0000800, 8'd128, 1'b0);
		drive_return_burst(8'h22);
		drive_return_burst(8'h11);
		wait_capture();

		golden = golden_burst(16'h56da, 28'h0000800, 8'h11);
		golden = golden_burst(golden, 28'h0000880, 8'h22);
		read_record();
		if(words[1] != FETCH_FLAG_CAPTURE_VALID || words[2] != 16'd1 ||
		   words[3] != golden)
			fail("first wrap-aligned fetch epoch mismatch");
		stable_signature = words[3];

		// The exact same address/data epoch advances sequence and stays stable.
		pulse_request(28'h0000880, 8'd128, 1'b0);
		drive_return_burst(8'h22);
		pulse_request(28'h0000800, 8'd128, 1'b0);
		drive_return_burst(8'h11);
		wait_capture();
		read_record();
		if(words[2] != 16'd2 || words[3] != stable_signature)
			fail("identical ordered fetch epoch did not remain stable");

		// Returned-data change in an otherwise identical address epoch changes
		// the combined address/data signature.
		golden = golden_burst(16'h56da, 28'h0000800, 8'h11);
		golden = golden_burst(golden, 28'h0000880, 8'h33);
		pulse_request(28'h0000880, 8'd128, 1'b0);
		drive_return_burst(8'h33);
		pulse_request(28'h0000800, 8'd128, 1'b0);
		drive_return_burst(8'h11);
		wait_capture();
		read_record();
		changed_signature = words[3];
		if(words[2] != 16'd3 || changed_signature != golden ||
		   changed_signature == stable_signature)
			fail("ordered returned-data change was not detected");

		// Swapping complete 16-bit lanes changes the folded-return signature;
		// the reducer is not permutation-invariant like a plain lane XOR.
		golden = golden_burst(16'h56da, 28'h0000800, 8'h11);
		golden = golden_burst_lane_swapped(golden, 28'h0000880, 8'h22);
		pulse_request(28'h0000880, 8'd128, 1'b0);
		drive_return_burst_lane_swapped(8'h22);
		pulse_request(28'h0000800, 8'd128, 1'b0);
		drive_return_burst(8'h11);
		wait_capture();
		read_record();
		if(words[2] != 16'd4 || words[3] != golden ||
		   words[3] == stable_signature)
			fail("16-bit return-lane permutation was not detected");

		// Accepted-address changes are part of the ordered signature even when
		// the returned data and burst order remain otherwise identical.
		golden = golden_burst(16'h56da, 28'h0000800, 8'h11);
		golden = golden_burst(golden, 28'h0000900, 8'h22);
		pulse_request(28'h0000900, 8'd128, 1'b0);
		drive_return_burst(8'h22);
		pulse_request(28'h0000800, 8'd128, 1'b0);
		drive_return_burst(8'h11);
		wait_capture();
		read_record();
		if(words[2] != 16'd5 || words[3] != golden ||
		   words[3] == stable_signature)
			fail("accepted-address change was not detected");

		// Partial reads reset cleanly.
		command_start(MAGIK_UIO_GET_RAW_SCALER_STATE);
		read_word(words[0]);
		read_word(words[1]);
		command_end();
		read_record();

		// A source publication during streaming cannot mutate the response.
		command_start(MAGIK_UIO_GET_RAW_SCALER_STATE);
		for(index = 0; index < 3; index = index + 1)
			read_word(immutable_words[index]);
		pulse_request(28'h0000880, 8'd128, 1'b0);
		drive_return_burst(8'h44);
		pulse_request(28'h0000800, 8'd128, 1'b0);
		drive_return_burst(8'h11);
		wait_capture();
		for(index = 3; index < 5; index = index + 1)
			read_word(immutable_words[index]);
		command_end();
		for(index = 0; index < 5; index = index + 1)
			words[index] = immutable_words[index];
		if(words[4] != response_crc(3))
			fail("immutable mid-read response CRC mismatch");

		// Sequence wrap remains a valid coherent capture.
		wait_capture();
		dut.snapshot_sequence = 16'hffff;
		pulse_request(28'h0000880, 8'd128, 1'b0);
		drive_return_burst(8'h55);
		pulse_request(28'h0000800, 8'd128, 1'b0);
		drive_return_burst(8'h11);
		wait_capture();
		read_record();
		if(words[1] != FETCH_FLAG_CAPTURE_VALID || words[2] != 16'd0)
			fail("capture sequence wrap lost valid evidence");

		// A new valid publication followed at the next source edge by a sticky
		// fault must not cancel in the toggle CDC and leave this older valid
		// snapshot readable. Fault publication uses its independent sticky
		// level and does not toggle the valid-generation channel.
		@(negedge clk_100m);
		dut.published_signature = 16'hcafe;
		dut.published_flags = FETCH_FLAG_CAPTURE_VALID[6:0];
		dut.source_generation = ~dut.source_generation;
		prior_generation = dut.source_generation;
		@(posedge clk_100m);
		pulse_request(28'h0000880, 8'd127, 1'b0);
		if(dut.source_generation != prior_generation)
			fail("fault publication reused the cancellable valid-generation toggle");
		wait_capture();
		read_record();
		if(words[1] != FETCH_FLAG_BAD_BURSTCOUNT || words[2] != 16'd0 ||
		   words[3] != 16'd0)
			fail("valid publication followed by an immediate fault stayed readable");
		apply_reset();

		// Reset during a partially returned accepted burst discards both the
		// transaction and prior publication; no mixed epoch may remain visible.
		pulse_request(28'h0000880, 8'd128, 1'b0);
		for(index = 0; index < 8; index = index + 1) begin
			@(negedge clk_100m);
			vbuf_readdatavalid = 1'b1;
			vbuf_readdata = beat_data(8'h66, index[7:0]);
			@(posedge clk_100m);
		end
		@(negedge clk_100m);
		vbuf_readdatavalid = 1'b0;
		vbuf_readdata = 128'd0;
		apply_reset();
		wait_capture();
		read_record();
		if(words[1] != 16'd0 || words[2] != 16'd0 || words[3] != 16'd0)
			fail("mid-burst reset retained ambiguous fetch evidence");

		// Each protocol defect publishes a sticky invalid record with zero
		// sequence/signature. Exercise all externally reachable fault classes.
		apply_reset();
		pulse_request(28'h0001000, 8'd128, 1'b0);
		apply_reset();
		read_record();
		if(words[1] != 0 || words[2] != 0 || words[3] != 0)
			fail("reset with outstanding work manufactured evidence");
		// If an ambiguous pre-reset return survives the reset boundary, the
		// empty post-reset scoreboard must invalidate it rather than hash it.
		@(negedge clk_100m);
		vbuf_readdatavalid = 1'b1;
		@(posedge clk_100m);
		@(negedge clk_100m);
		vbuf_readdatavalid = 1'b0;
		wait_capture();
		read_record();
		if(words[1] != FETCH_FLAG_UNEXPECTED_RETURN || words[2] != 0 || words[3] != 0)
			fail("unexpected return did not fail closed");

		apply_reset();
		pulse_request(28'h0001000, 8'd127, 1'b0);
		wait_capture();
		read_record();
		if(words[1] != FETCH_FLAG_BAD_BURSTCOUNT || words[2] != 0 || words[3] != 0)
			fail("bad burstcount did not fail closed");

		apply_reset();
		pulse_request(28'h0001000, 8'd128, 1'b0);
		pulse_request(28'h0001080, 8'd128, 1'b0);
		pulse_request(28'h0001100, 8'd128, 1'b0);
		wait_capture();
		read_record();
		if(words[1] != FETCH_FLAG_FIFO_OVERFLOW || words[2] != 0 || words[3] != 0)
			fail("FIFO overflow did not fail closed");

		apply_reset();
		pulse_request(28'h0003000, 8'd128, 1'b0);
		drive_return_burst(8'h10);
		pulse_request(28'h0002000, 8'd128, 1'b0);
		pulse_request(28'h0001000, 8'd128, 1'b0);
		wait_capture();
		read_record();
		if(words[1] != FETCH_FLAG_EPOCH_OVERLAP || words[2] != 0 || words[3] != 0)
			fail("epoch overlap did not fail closed");

		apply_reset();
		pulse_request(28'h0003000, 8'd128, 1'b0);
		drive_return_burst(8'h10);
		pulse_request(28'h0002000, 8'd128, 1'b0);
		dut.return_phase = 7'd1;
		@(negedge clk_100m);
		vbuf_readdatavalid = 1'b1;
		@(posedge clk_100m);
		@(negedge clk_100m);
		vbuf_readdatavalid = 1'b0;
		wait_capture();
		read_record();
		if(words[1] != FETCH_FLAG_BAD_RETURN_PHASE || words[2] != 0 || words[3] != 0)
			fail("bad return phase did not fail closed");

		apply_reset();
		dut.fifo_count = 2'd3;
		@(posedge clk_100m);
		wait_capture();
		read_record();
		if(words[1] != FETCH_FLAG_COUNTER_OVERFLOW || words[2] != 0 || words[3] != 0)
			fail("counter overflow did not fail closed");

		$display("PASS: scaler fetch ordered-signature observer and fail-closed protocol");
		$finish;
	end
endmodule
