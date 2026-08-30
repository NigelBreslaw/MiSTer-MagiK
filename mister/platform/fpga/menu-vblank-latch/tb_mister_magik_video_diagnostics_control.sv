// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

`timescale 1ns/1ps

module tb_mister_magik_video_diagnostics_control;
	`include "mister_magik_video_diagnostics_protocol.svh"

	reg clk_100m = 1'b0;
	reg clk_sys = 1'b0;
	reg reset_req = 1'b1;
	reg [27:0] vbuf_address = 28'd0;
	reg [7:0] vbuf_burstcount = 8'd128;
	reg vbuf_waitrequest = 1'b0;
	reg vbuf_readdatavalid = 1'b0;
	reg vbuf_read = 1'b0;
	reg [15:0] scaler_diag_state = 16'd0;
	reg io_uio = 1'b0;
	reg io_strobe = 1'b0;
	reg [15:0] io_din = 16'd0;
	wire response_valid;
	wire [15:0] response_data;
	reg [15:0] words [0:3];
	reg [15:0] prior_words [0:3];
	integer index;

	mister_magik_scaler_fetch_liveness_state #(
		.WATCHDOG_LIMIT(24'd20),
		.RESET_QUALIFY_LIMIT(3'd4)
	) dut (
		.clk_100m(clk_100m),
		.clk_sys(clk_sys),
		.reset_req(reset_req),
		.vbuf_address(vbuf_address),
		.vbuf_burstcount(vbuf_burstcount),
		.vbuf_waitrequest(vbuf_waitrequest),
		.vbuf_readdatavalid(vbuf_readdatavalid),
		.vbuf_read(vbuf_read),
		.scaler_diag_state(scaler_diag_state),
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
		integer word_index;
		reg [15:0] value;
		begin
			value = 16'hffff;
			value = crc16_word(value,
				{8'd0, MAGIK_UIO_GET_SCALER_FETCH_LIVENESS_STATE});
			value = crc16_word(value, MAGIK_SCALER_FETCH_LIVENESS_STATE_SCHEMA);
			value = crc16_word(value,
				MAGIK_SCALER_FETCH_LIVENESS_STATE_WORDS - 1'd1);
			for(word_index = 0;
				word_index < MAGIK_SCALER_FETCH_LIVENESS_STATE_CRC_WORD;
				word_index = word_index + 1)
				value = crc16_word(value, words[word_index]);
			response_crc = value;
		end
	endfunction

	task automatic command_start;
		begin
			repeat(16) @(posedge clk_sys);
			@(negedge clk_sys);
			io_uio = 1'b1;
			io_strobe = 1'b1;
			io_din = {8'd0, MAGIK_UIO_GET_SCALER_FETCH_LIVENESS_STATE};
			#1;
			if(!response_valid ||
				response_data != MAGIK_SCALER_FETCH_LIVENESS_STATE_MAGIC)
				fail("0x68 magic response mismatch");
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
				fail("0x68 response ended before fixed word count");
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
			command_start();
			for(index = 0; index < MAGIK_SCALER_FETCH_LIVENESS_STATE_WORDS;
				index = index + 1)
				read_word(words[index]);
			command_end();
			if(words[0] != MAGIK_SCALER_FETCH_LIVENESS_STATE_SCHEMA)
				fail("schema mismatch");
			if(words[MAGIK_SCALER_FETCH_LIVENESS_STATE_CRC_WORD] != response_crc())
				fail("response CRC mismatch");
			if(!(words[1] & (MAGIK_SCALER_FETCH_LIVENESS_STATE_FLAG_FIRST_STALL_VALID |
				MAGIK_SCALER_FETCH_LIVENESS_STATE_FLAG_OBSERVER_FAULT)) &&
				words[MAGIK_SCALER_FETCH_LIVENESS_STATE_STATE_WORD][15])
				fail("reserved live-state bit set");
		end
	endtask

	task automatic drive_accept(input [27:0] address);
		begin
			@(negedge clk_100m);
			vbuf_address = address;
			vbuf_burstcount = 8'd128;
			vbuf_waitrequest = 1'b0;
			vbuf_read = 1'b1;
			@(posedge clk_100m);
			@(negedge clk_100m);
			vbuf_read = 1'b0;
		end
	endtask

	task automatic drive_return_beats(input integer count);
		integer beat;
		begin
			for(beat = 0; beat < count; beat = beat + 1) begin
				@(negedge clk_100m);
				vbuf_readdatavalid = 1'b1;
				@(posedge clk_100m);
			end
			@(negedge clk_100m);
			vbuf_readdatavalid = 1'b0;
		end
	endtask

	task automatic finish_return_with_accept(input [27:0] address);
		integer beat;
		begin
			for(beat = 0; beat < 120; beat = beat + 1) begin
				@(negedge clk_100m);
				vbuf_readdatavalid = 1'b1;
				if(beat == 119) begin
					vbuf_address = address;
					vbuf_burstcount = 8'd128;
					vbuf_waitrequest = 1'b0;
					vbuf_read = 1'b1;
				end
				@(posedge clk_100m);
			end
			@(negedge clk_100m);
			vbuf_readdatavalid = 1'b0;
			vbuf_read = 1'b0;
		end
	endtask

	initial begin
		reg [3:0] first_sequence;
		reg [3:0] second_sequence;
		integer qualified_attempt;

		// The observer publishes while reset is held and does not claim a
		// qualified record before synchronized reset-low qualification.
		repeat(20) @(posedge clk_100m);
		read_record();
		if(!(words[1] & MAGIK_SCALER_FETCH_LIVENESS_STATE_FLAG_RESET_LEVEL))
			fail("reset level not reported");
		if(words[1] & MAGIK_SCALER_FETCH_LIVENESS_STATE_FLAG_RECORD_VALID)
			fail("startup record incorrectly valid");
		first_sequence = (words[MAGIK_SCALER_FETCH_LIVENESS_STATE_PUBLICATION_SEQUENCE_WORD] >>
			MAGIK_SCALER_FETCH_LIVENESS_STATE_PUBLICATION_SEQUENCE_BIT) &
			MAGIK_SCALER_FETCH_LIVENESS_STATE_PUBLICATION_SEQUENCE_MASK;
		read_record();
		second_sequence = (words[MAGIK_SCALER_FETCH_LIVENESS_STATE_PUBLICATION_SEQUENCE_WORD] >>
			MAGIK_SCALER_FETCH_LIVENESS_STATE_PUBLICATION_SEQUENCE_BIT) &
			MAGIK_SCALER_FETCH_LIVENESS_STATE_PUBLICATION_SEQUENCE_MASK;
		if(second_sequence == first_sequence)
			fail("publication heartbeat did not advance during reset");

		// Accepted obligations and return phase survive reset. The final beat of
		// the first burst simultaneously accepts the wrap-marked second burst.
		drive_accept(28'h0800000);
		drive_return_beats(8);
		reset_req = 1'b0;
		repeat(6) @(posedge clk_100m);
		finish_return_with_accept(28'h0000000);
		drive_return_beats(128);
		// Freeze an output scheduler snapshot: sREAD/sCOPY, both levels full,
		// copy active past adturn/next-word, but no terminal predicate.
		@(negedge clk_100m);
		scaler_diag_state = 16'h38aa;

		// Leave the qualified, empty boundary idle long enough to freeze the
		// exact no-request observation. Consume a bounded pending publication:
		// its acknowledgement lets the source publish the already-frozen tuple.
		repeat(24) @(posedge clk_100m);
		for(qualified_attempt = 0;
			qualified_attempt < 4 &&
			!(words[1] & MAGIK_SCALER_FETCH_LIVENESS_STATE_FLAG_RECORD_VALID &&
				words[1] & MAGIK_SCALER_FETCH_LIVENESS_STATE_FLAG_NORMAL_LIVENESS_SEEN &&
				words[1] & MAGIK_SCALER_FETCH_LIVENESS_STATE_FLAG_FIRST_STALL_VALID);
			qualified_attempt = qualified_attempt + 1)
			read_record();
		if(!(words[1] & MAGIK_SCALER_FETCH_LIVENESS_STATE_FLAG_RECORD_VALID))
			fail("qualified record not valid");
		if(!(words[1] & MAGIK_SCALER_FETCH_LIVENESS_STATE_FLAG_NORMAL_LIVENESS_SEEN))
			fail("wrap-marked complete burst did not establish normal liveness");
		if(!(words[1] & MAGIK_SCALER_FETCH_LIVENESS_STATE_FLAG_FIRST_STALL_VALID))
			fail("first stall was not frozen");
		if(words[1] & MAGIK_SCALER_FETCH_LIVENESS_STATE_FLAG_OBSERVER_FAULT)
			fail("reset-retained obligation produced observer fault");
		if(!(words[1] & MAGIK_SCALER_FETCH_LIVENESS_STATE_FLAG_NO_REQUEST_SEEN))
			fail("no-request classification flag was not frozen");
		if(words[MAGIK_SCALER_FETCH_LIVENESS_STATE_STATE_WORD] != 16'h38aa)
			fail("wrong output-scheduler gate snapshot");
		for(index = 0; index < MAGIK_SCALER_FETCH_LIVENESS_STATE_WORDS;
			index = index + 1)
			prior_words[index] = words[index];

		// A later good burst changes rolling progress but cannot overwrite the
		// frozen cause, phase, FIFO depth, address fold, or temporal identity.
		drive_accept(28'h0000080);
		drive_return_beats(128);
		read_record();
		if(words[MAGIK_SCALER_FETCH_LIVENESS_STATE_STATE_WORD] !=
				prior_words[MAGIK_SCALER_FETCH_LIVENESS_STATE_STATE_WORD])
			fail("sticky first-stall evidence changed");
		if((words[MAGIK_SCALER_FETCH_LIVENESS_STATE_PUBLICATION_SEQUENCE_WORD] & 16'hf000) ==
				(prior_words[MAGIK_SCALER_FETCH_LIVENESS_STATE_PUBLICATION_SEQUENCE_WORD] & 16'hf000))
			fail("publication sequence stopped after frozen event");
		for(index = 0; index < MAGIK_SCALER_FETCH_LIVENESS_STATE_WORDS;
			index = index + 1)
			prior_words[index] = words[index];

		// Reset remains observable but cannot erase either part of the frozen
		// identity/state snapshot now sharing the stopped watchdog bank.
		reset_req = 1'b1;
		repeat(6) @(posedge clk_100m);
		read_record();
		if(words[MAGIK_SCALER_FETCH_LIVENESS_STATE_STATE_WORD] !=
				prior_words[MAGIK_SCALER_FETCH_LIVENESS_STATE_STATE_WORD])
			fail("reset erased sticky first-stall evidence");
		reset_req = 1'b0;
		repeat(6) @(posedge clk_100m);

		// Legacy command is deliberately unsupported by the replacement RBF.
		@(negedge clk_sys);
		io_uio = 1'b1;
		io_strobe = 1'b1;
		io_din = {8'd0, MAGIK_UIO_GET_RAW_SCALER_STATE};
		#1;
		if(response_valid)
			fail("retired 0x67 responder remained active");
		@(posedge clk_sys);
		io_uio = 1'b0;
		io_strobe = 1'b0;

		$display("PASS: scaler fetch liveness observer");
		$finish;
	end
endmodule
