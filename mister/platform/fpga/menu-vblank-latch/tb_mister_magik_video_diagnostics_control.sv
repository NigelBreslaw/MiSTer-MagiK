// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

`timescale 1ns/1ps
`default_nettype none

module tb_mister_magik_video_diagnostics_control;
`include "mister_magik_video_diagnostics_protocol.svh"

	reg clk_sys = 1'b0;
	always #5 clk_sys = ~clk_sys;
	reg hdmi_tx_clk = 1'b0;
	always #7 hdmi_tx_clk = ~hdmi_tx_clk;

	reg io_uio = 1'b0;
	reg io_strobe = 1'b0;
	reg [15:0] io_din = 16'd0;
	reg hdmi_pll_locked = 1'b0;
	reg hdmi_out_vs = 1'b0;
	reg hdmi_out_de = 1'b0;
	reg [23:0] hdmi_out_d = 24'd0;
	wire response_valid;
	wire [15:0] response_data;

	mister_magik_hdmi_lock_evidence dut (
		.clk_sys(clk_sys),
		.hdmi_tx_clk(hdmi_tx_clk),
		.io_uio(io_uio),
		.io_strobe(io_strobe),
		.io_din(io_din),
		.hdmi_pll_locked(hdmi_pll_locked),
		.hdmi_out_vs(hdmi_out_vs),
		.hdmi_out_de(hdmi_out_de),
		.hdmi_out_d(hdmi_out_d),
		.response_valid(response_valid),
		.response_data(response_data)
	);

	function automatic [15:0] crc_byte;
		input [15:0] crc_in;
		input [7:0] data;
		integer bit_index;
		reg [15:0] value;
		begin
			value = crc_in ^ {data, 8'd0};
			for(bit_index = 0; bit_index < 8; bit_index = bit_index + 1)
				value = value[15] ? ((value << 1) ^ 16'h1021) : value << 1;
			crc_byte = value;
		end
	endfunction

	function automatic [15:0] crc_word;
		input [15:0] crc_in;
		input [15:0] data;
		begin
			crc_word = crc_byte(crc_byte(crc_in, data[15:8]), data[7:0]);
		end
	endfunction

	task automatic close_command;
		begin
			@(negedge clk_sys); io_uio = 1'b0; io_strobe = 1'b0;
			@(negedge clk_sys);
		end
	endtask

	task automatic read_activity_snapshot_while_frame_completes;
		input [15:0] expected_flags;
		input [7:0] expected_no_de;
		input [7:0] expected_all_zero;
		input [7:0] expected_nonzero;
		begin
			io_uio = 1'b1;
			io_din = 16'h0061; io_strobe = 1'b1;
			#1 if(!response_valid || response_data != MAGIK_HDMI_OUTPUT_ACTIVITY_MAGIC)
				$fatal(1, "missing activity magic before concurrent frame");
			@(negedge clk_sys); io_strobe = 1'b0;
			complete_output_frame(1'b1, 1'b1);
			crc = MAGIK_HDMI_OUTPUT_ACTIVITY_HEADER_CRC;
			for(index = 0; index < MAGIK_HDMI_OUTPUT_ACTIVITY_WORDS; index = index + 1) begin
				@(negedge clk_sys); io_strobe = 1'b1;
				#1 words[index] = response_data;
				if(index < MAGIK_HDMI_OUTPUT_ACTIVITY_CRC_WORD)
					crc = crc_word(crc, response_data);
				@(negedge clk_sys); io_strobe = 1'b0;
			end
			close_command();
			if(words[MAGIK_HDMI_OUTPUT_ACTIVITY_FLAGS_WORD] != expected_flags ||
			   words[MAGIK_HDMI_OUTPUT_ACTIVITY_NO_DE_COUNT_WORD] != {8'd0, expected_no_de} ||
			   words[MAGIK_HDMI_OUTPUT_ACTIVITY_DE_ALL_ZERO_COUNT_WORD] !=
					{8'd0, expected_all_zero} ||
			   words[MAGIK_HDMI_OUTPUT_ACTIVITY_DE_HAS_NONZERO_COUNT_WORD] !=
					{8'd0, expected_nonzero} ||
			   words[MAGIK_HDMI_OUTPUT_ACTIVITY_CRC_WORD] != crc)
				$fatal(1, "activity command-start snapshot changed mid-read");
		end
	endtask

	task automatic drive_synchronized_lock;
		input value;
		begin
			hdmi_pll_locked = value;
			while(dut.control_pll_lock_sys !== value) @(negedge clk_sys);
			if(value)
				while(dut.lock_armed !== 1'b1) @(negedge clk_sys);
		end
	endtask

	task automatic pulse_output_vs;
		begin
			@(negedge hdmi_tx_clk); hdmi_out_vs = 1'b1;
			@(negedge hdmi_tx_clk); hdmi_out_vs = 1'b0;
		end
	endtask

	task automatic complete_output_frame;
		input saw_de;
		input saw_nonzero;
		begin
			@(negedge hdmi_tx_clk);
			hdmi_out_vs = 1'b0;
			hdmi_out_de = saw_de;
			hdmi_out_d = saw_nonzero ? 24'h010203 : 24'd0;
			@(negedge hdmi_tx_clk);
			hdmi_out_de = 1'b0;
			hdmi_out_d = 24'd0;
			pulse_output_vs();
			repeat(8) @(negedge clk_sys);
		end
	endtask

	integer index;
	reg [15:0] words [0:5];
	reg [15:0] crc;
	reg [15:0] armed_flags;
	reg [15:0] lost_flags;
	task automatic read_evidence;
		input [15:0] expected_flags;
		input [15:0] expected_loss_count;
		begin
			io_uio = 1'b1;
			io_din = 16'h0060; io_strobe = 1'b1;
			#1 if(!response_valid || response_data != MAGIK_HDMI_EVIDENCE_MAGIC)
				$fatal(1, "missing HDMI lock evidence magic");
			@(negedge clk_sys); io_strobe = 1'b0;
			#1 if(response_valid) $fatal(1, "response remained valid between strobes");
			crc = MAGIK_HDMI_EVIDENCE_HEADER_CRC;
			for(index = 0; index < MAGIK_HDMI_EVIDENCE_WORDS; index = index + 1) begin
				@(negedge clk_sys); io_din = 16'd0; io_strobe = 1'b1;
				#1;
				if(!response_valid) $fatal(1, "HDMI evidence ended at word %0d", index);
				words[index] = response_data;
				if(index < MAGIK_HDMI_EVIDENCE_CRC_WORD)
					crc = crc_word(crc, response_data);
				@(negedge clk_sys); io_strobe = 1'b0;
				#1 if(response_valid) $fatal(1, "word response remained valid between strobes");
			end
			@(negedge clk_sys); io_strobe = 1'b1;
			#1 if(response_valid) $fatal(1, "HDMI evidence exceeded fixed word count");
			@(negedge clk_sys); io_strobe = 1'b0;
			close_command();
			if(words[MAGIK_HDMI_EVIDENCE_SCHEMA_WORD] != MAGIK_HDMI_EVIDENCE_SCHEMA ||
			   words[MAGIK_HDMI_EVIDENCE_FLAGS_WORD] != expected_flags ||
			   words[MAGIK_HDMI_EVIDENCE_LOCK_LOSS_COUNT_WORD] != expected_loss_count ||
			   words[MAGIK_HDMI_EVIDENCE_CRC_WORD] != crc)
				$fatal(1, "HDMI evidence mismatch flags=%h count=%h crc=%h/%h",
					words[MAGIK_HDMI_EVIDENCE_FLAGS_WORD],
					words[MAGIK_HDMI_EVIDENCE_LOCK_LOSS_COUNT_WORD],
					words[MAGIK_HDMI_EVIDENCE_CRC_WORD], crc);
		end
	endtask

	task automatic read_output_activity;
		input [15:0] expected_flags;
		input [7:0] expected_no_de;
		input [7:0] expected_all_zero;
		input [7:0] expected_nonzero;
		begin
			io_uio = 1'b1;
			io_din = 16'h0061; io_strobe = 1'b1;
			#1 if(!response_valid || response_data != MAGIK_HDMI_OUTPUT_ACTIVITY_MAGIC)
				$fatal(1, "missing HDMI output activity magic");
			@(negedge clk_sys); io_strobe = 1'b0;
			#1 if(response_valid) $fatal(1, "activity response remained valid between strobes");
			crc = MAGIK_HDMI_OUTPUT_ACTIVITY_HEADER_CRC;
			for(index = 0; index < MAGIK_HDMI_OUTPUT_ACTIVITY_WORDS; index = index + 1) begin
				@(negedge clk_sys); io_din = 16'd0; io_strobe = 1'b1;
				#1;
				if(!response_valid) $fatal(1, "HDMI activity ended at word %0d", index);
				words[index] = response_data;
				if(index < MAGIK_HDMI_OUTPUT_ACTIVITY_CRC_WORD)
					crc = crc_word(crc, response_data);
				@(negedge clk_sys); io_strobe = 1'b0;
				#1 if(response_valid) $fatal(1, "activity word remained valid between strobes");
			end
			@(negedge clk_sys); io_strobe = 1'b1;
			#1 if(response_valid) $fatal(1, "HDMI activity exceeded fixed word count");
			@(negedge clk_sys); io_strobe = 1'b0;
			close_command();
			if(words[MAGIK_HDMI_OUTPUT_ACTIVITY_SCHEMA_WORD] !=
					MAGIK_HDMI_OUTPUT_ACTIVITY_SCHEMA ||
			   words[MAGIK_HDMI_OUTPUT_ACTIVITY_FLAGS_WORD] != expected_flags ||
			   words[MAGIK_HDMI_OUTPUT_ACTIVITY_NO_DE_COUNT_WORD] != {8'd0, expected_no_de} ||
			   words[MAGIK_HDMI_OUTPUT_ACTIVITY_DE_ALL_ZERO_COUNT_WORD] !=
					{8'd0, expected_all_zero} ||
			   words[MAGIK_HDMI_OUTPUT_ACTIVITY_DE_HAS_NONZERO_COUNT_WORD] !=
					{8'd0, expected_nonzero} ||
			   words[MAGIK_HDMI_OUTPUT_ACTIVITY_CRC_WORD] != crc)
				$fatal(1, "HDMI activity mismatch flags=%h counts=%h/%h/%h crc=%h/%h",
					words[MAGIK_HDMI_OUTPUT_ACTIVITY_FLAGS_WORD],
					words[MAGIK_HDMI_OUTPUT_ACTIVITY_NO_DE_COUNT_WORD],
					words[MAGIK_HDMI_OUTPUT_ACTIVITY_DE_ALL_ZERO_COUNT_WORD],
					words[MAGIK_HDMI_OUTPUT_ACTIVITY_DE_HAS_NONZERO_COUNT_WORD],
					words[MAGIK_HDMI_OUTPUT_ACTIVITY_CRC_WORD], crc);
		end
	endtask

	task automatic read_armed_snapshot_while_lock_falls;
		begin
			io_uio = 1'b1; io_din = 16'h0060; io_strobe = 1'b1;
			#1 if(!response_valid || response_data != MAGIK_HDMI_EVIDENCE_MAGIC)
				$fatal(1, "missing magic before concurrent loss");
			@(negedge clk_sys); io_strobe = 1'b0; hdmi_pll_locked = 1'b0;
			crc = MAGIK_HDMI_EVIDENCE_HEADER_CRC;
			for(index = 0; index < MAGIK_HDMI_EVIDENCE_WORDS; index = index + 1) begin
				@(negedge clk_sys); io_strobe = 1'b1;
				#1 words[index] = response_data;
				if(index < MAGIK_HDMI_EVIDENCE_CRC_WORD)
					crc = crc_word(crc, response_data);
				@(negedge clk_sys); io_strobe = 1'b0;
			end
			close_command();
			if(words[MAGIK_HDMI_EVIDENCE_FLAGS_WORD] != armed_flags ||
			   words[MAGIK_HDMI_EVIDENCE_LOCK_LOSS_COUNT_WORD] != 16'd0 ||
			   words[MAGIK_HDMI_EVIDENCE_CRC_WORD] != crc)
				$fatal(1, "command-start snapshot changed during concurrent loss");
		end
	endtask

	integer opcode;
	initial begin
		armed_flags = MAGIK_HDMI_EVIDENCE_FLAG_LOCK_SEEN_HIGH |
			MAGIK_HDMI_EVIDENCE_FLAG_LOCK_ARMED |
			MAGIK_HDMI_EVIDENCE_FLAG_LOCK_CURRENT;
		lost_flags = MAGIK_HDMI_EVIDENCE_FLAG_LOCK_SEEN_HIGH |
			MAGIK_HDMI_EVIDENCE_FLAG_LOCK_ARMED |
			MAGIK_HDMI_EVIDENCE_FLAG_LOCK_EVER_LOST;

		repeat(4) @(negedge clk_sys);
		if(MAGIK_HDMI_EVIDENCE_HEADER_CRC != 16'h109f)
			$fatal(1, "unexpected generated HDMI evidence header CRC");
		if(MAGIK_HDMI_OUTPUT_ACTIVITY_HEADER_CRC != 16'hda08)
			$fatal(1, "unexpected generated HDMI output activity header CRC");

		// The responder must ignore all latch-owned and retired commands.
		for(opcode = 8'h57; opcode <= 8'h5f; opcode = opcode + 1) begin
			io_uio = 1'b1;
			io_din = opcode[15:0]; io_strobe = 1'b1;
			#1 if(response_valid) $fatal(1, "lock evidence responded to opcode %h", opcode);
			close_command();
		end
		io_uio = 1'b0; io_din = 16'h0060; io_strobe = 1'b1;
		#1 if(response_valid) $fatal(1, "lock evidence responded outside UIO");
		@(negedge clk_sys); io_strobe = 1'b0;

		// A different command cannot become selected by changing io_din later.
		io_uio = 1'b1; io_din = 16'h0062; io_strobe = 1'b1;
		#1 if(response_valid) $fatal(1, "unknown command unexpectedly selected");
		@(negedge clk_sys); io_strobe = 1'b0;
		@(negedge clk_sys); io_din = 16'h0060; io_strobe = 1'b1;
		#1 if(response_valid) $fatal(1, "active command morphed into HDMI evidence");
		close_command();

		read_evidence(16'd0, 16'd0);
		read_output_activity(16'd0, 8'd0, 8'd0, 8'd0);

		// The first rising VS only arms and discards the partial interval.
		pulse_output_vs();
		repeat(8) @(negedge clk_sys);
		read_output_activity(16'd0, 8'd0, 8'd0, 8'd0);

		complete_output_frame(1'b0, 1'b0);
		read_output_activity(MAGIK_HDMI_OUTPUT_ACTIVITY_FLAG_FRAME_VALID,
			8'd1, 8'd0, 8'd0);
		complete_output_frame(1'b1, 1'b0);
		read_output_activity(MAGIK_HDMI_OUTPUT_ACTIVITY_FLAG_FRAME_VALID,
			8'd1, 8'd1, 8'd0);
		complete_output_frame(1'b1, 1'b1);
		read_output_activity(MAGIK_HDMI_OUTPUT_ACTIVITY_FLAG_FRAME_VALID,
			8'd1, 8'd1, 8'd1);

		// Nonzero RGB outside DE must not be reported as active nonzero video.
		@(negedge hdmi_tx_clk); hdmi_out_d = 24'hffffff;
		@(negedge hdmi_tx_clk); hdmi_out_d = 24'd0;
		pulse_output_vs();
		repeat(8) @(negedge clk_sys);
		read_output_activity(MAGIK_HDMI_OUTPUT_ACTIVITY_FLAG_FRAME_VALID,
			8'd2, 8'd1, 8'd1);

		// A long VS level is one edge, not multiple completed frames.
		@(negedge hdmi_tx_clk); hdmi_out_de = 1'b1; hdmi_out_d = 24'd0;
		@(negedge hdmi_tx_clk); hdmi_out_de = 1'b0;
		hdmi_out_vs = 1'b1;
		repeat(4) @(negedge hdmi_tx_clk);
		hdmi_out_vs = 1'b0;
		repeat(8) @(negedge clk_sys);
		read_output_activity(MAGIK_HDMI_OUTPUT_ACTIVITY_FLAG_FRAME_VALID,
			8'd2, 8'd2, 8'd1);

		// A command snapshots atomically even if another completed frame arrives
		// before the payload words are streamed.
		read_activity_snapshot_while_frame_completes(
			MAGIK_HDMI_OUTPUT_ACTIVITY_FLAG_FRAME_VALID, 8'd2, 8'd2, 8'd1);
		read_output_activity(MAGIK_HDMI_OUTPUT_ACTIVITY_FLAG_FRAME_VALID,
			8'd2, 8'd2, 8'd2);

		// Counters are explicitly modulo four-bit epochs; wrapping remains a
		// valid snapshot for host-side modular delta calculation.
		@(negedge clk_sys); dut.output_no_de_count = 4'hf;
		complete_output_frame(1'b0, 1'b0);
		read_output_activity(MAGIK_HDMI_OUTPUT_ACTIVITY_FLAG_FRAME_VALID,
			8'd0, 8'd2, 8'd2);

		// The source classifier cannot emit two classes for one frame. If CDC
		// capture nevertheless sees simultaneous channels, retain a sticky
		// integrity flag instead of silently presenting coherent evidence.
		@(negedge hdmi_tx_clk);
		dut.output_de_all_zero_toggle = !dut.output_de_all_zero_toggle;
		dut.output_de_has_nonzero_toggle = !dut.output_de_has_nonzero_toggle;
		repeat(8) @(negedge clk_sys);
		read_output_activity(
			MAGIK_HDMI_OUTPUT_ACTIVITY_FLAG_FRAME_VALID |
			MAGIK_HDMI_OUTPUT_ACTIVITY_FLAG_COUNTER_COLLISION,
			8'd0, 8'd3, 8'd3);

		// An aborted activity transaction restarts from its own schema and CRC
		// seed without perturbing the permanent 0x60 lock record.
		io_uio = 1'b1;
		@(negedge clk_sys); io_din = 16'h0061; io_strobe = 1'b1;
		@(negedge clk_sys); io_strobe = 1'b0;
		@(negedge clk_sys); io_strobe = 1'b1;
		#1 if(response_data != MAGIK_HDMI_OUTPUT_ACTIVITY_SCHEMA)
			$fatal(1, "activity transaction did not start at schema");
		close_command();
		read_output_activity(
			MAGIK_HDMI_OUTPUT_ACTIVITY_FLAG_FRAME_VALID |
			MAGIK_HDMI_OUTPUT_ACTIVITY_FLAG_COUNTER_COLLISION,
			8'd0, 8'd3, 8'd3);

		// A raw pulse sampled by the first stage exactly once produces one high
		// sample at the synchronized stage. It records that lock was seen but
		// must not arm the loss counter.
		@(negedge clk_sys); hdmi_pll_locked = 1'b1;
		@(negedge clk_sys); hdmi_pll_locked = 1'b0;
		while(dut.control_pll_lock_sys !== 1'b1) @(negedge clk_sys);
		while(dut.control_pll_lock_sys !== 1'b0) @(negedge clk_sys);
		read_evidence(MAGIK_HDMI_EVIDENCE_FLAG_LOCK_SEEN_HIGH, 16'd0);

		drive_synchronized_lock(1'b1);
		read_evidence(armed_flags, 16'd0);
		read_armed_snapshot_while_lock_falls();
		read_evidence(lost_flags, 16'd1);

		// Recover, then start on the first destination half-cycle after another
		// synchronized fall,
		// before sticky state receives its next edge. The command snapshot must
		// include the same pending loss transition through *_next.
		drive_synchronized_lock(1'b1);
		drive_synchronized_lock(1'b0);
		if(dut.lock_loss_event !== 1'b1)
			$fatal(1, "test missed the uncommitted synchronized loss edge");
		read_evidence(lost_flags, 16'd2);

		// Recovery changes only the live-current flag. A second loss increments
		// the sticky counter and retains the first-fault evidence.
		drive_synchronized_lock(1'b1);
		read_evidence(armed_flags | MAGIK_HDMI_EVIDENCE_FLAG_LOCK_EVER_LOST, 16'd2);
		drive_synchronized_lock(1'b0);
		read_evidence(lost_flags, 16'd3);

		// An aborted transaction must restart from schema word zero.
		io_uio = 1'b1;
		@(negedge clk_sys); io_din = 16'h0060; io_strobe = 1'b1;
		@(negedge clk_sys); io_strobe = 1'b0;
		@(negedge clk_sys); io_strobe = 1'b1;
		#1 if(response_data != MAGIK_HDMI_EVIDENCE_SCHEMA)
			$fatal(1, "first transaction did not start at schema");
		close_command();
		read_evidence(lost_flags, 16'd3);

		// Exercise saturation without spending 65,535 simulated transitions.
		drive_synchronized_lock(1'b1);
		@(negedge clk_sys);
		dut.lock_loss_count = 16'hffff;
		dut.lock_ever_lost = 1'b1;
		drive_synchronized_lock(1'b0);
		read_evidence(lost_flags |
			MAGIK_HDMI_EVIDENCE_FLAG_LOCK_LOSS_COUNT_OVERFLOW, 16'hffff);

		$display("HDMI lock evidence tests passed");
		$finish;
	end
endmodule

`default_nettype wire
