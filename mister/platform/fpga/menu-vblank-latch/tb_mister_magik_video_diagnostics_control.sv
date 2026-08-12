// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

`timescale 1ns/1ps
`default_nettype none

module tb_mister_magik_video_diagnostics_control;
`include "mister_magik_video_diagnostics_protocol.svh"

	reg clk_sys = 1'b0;
	always #5 clk_sys = ~clk_sys;

	reg io_uio = 1'b0;
	reg io_strobe = 1'b0;
	reg [15:0] io_din = 16'd0;
	reg hdmi_pll_locked = 1'b0;
	wire response_valid;
	wire [15:0] response_data;

	mister_magik_hdmi_lock_evidence dut (
		.clk_sys(clk_sys),
		.io_uio(io_uio),
		.io_strobe(io_strobe),
		.io_din(io_din),
		.hdmi_pll_locked(hdmi_pll_locked),
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

	task automatic drive_synchronized_lock;
		input value;
		begin
			hdmi_pll_locked = value;
			while(dut.control_pll_lock_sys !== value) @(negedge clk_sys);
			if(value)
				while(dut.lock_armed !== 1'b1) @(negedge clk_sys);
		end
	endtask

	integer index;
	reg [15:0] words [0:3];
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
		io_uio = 1'b1; io_din = 16'h0061; io_strobe = 1'b1;
		#1 if(response_valid) $fatal(1, "unknown command unexpectedly selected");
		@(negedge clk_sys); io_strobe = 1'b0;
		@(negedge clk_sys); io_din = 16'h0060; io_strobe = 1'b1;
		#1 if(response_valid) $fatal(1, "active command morphed into HDMI evidence");
		close_command();

		read_evidence(16'd0, 16'd0);
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
