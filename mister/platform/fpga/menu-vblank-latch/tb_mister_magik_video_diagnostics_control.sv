// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

`timescale 1ns/1ps
`default_nettype none

module tb_mister_magik_video_diagnostics_control;
	reg clk_hdmi = 1'b0;
	reg clk_sys = 1'b0;
	reg reset_active = 1'b1;
	reg [31:0] copy_state = 32'd0;
	reg copy_generation = 1'b0;
	reg io_uio = 1'b0;
	reg io_strobe = 1'b0;
	reg [15:0] io_din = 16'd0;
	wire response_valid;
	wire [15:0] response_data;
	reg [15:0] record [0:3];
	reg [15:0] saved [0:3];
	reg [15:0] expected_crc;
	integer command;
	integer index;

	always #5 clk_sys = ~clk_sys;
	always #7 clk_hdmi = ~clk_hdmi;

	`include "mister_magik_video_diagnostics_protocol.svh"

	mister_magik_raw_scaler_diagnostic dut (
		.clk_hdmi(clk_hdmi), .clk_sys(clk_sys), .reset_active(reset_active),
		.copy_state(copy_state), .copy_generation(copy_generation),
		.io_uio(io_uio), .io_strobe(io_strobe), .io_din(io_din),
		.response_valid(response_valid), .response_data(response_data)
	);

	function automatic [15:0] crc_byte;
		input [15:0] current;
		input [7:0] value;
		integer bit_index;
		reg [15:0] result;
		begin
			result = current ^ {value, 8'h00};
			for(bit_index = 0; bit_index < 8; bit_index = bit_index + 1)
				result = result[15] ? ((result << 1) ^ 16'h1021) : (result << 1);
			crc_byte = result;
		end
	endfunction

	function automatic [15:0] crc_word;
		input [15:0] current;
		input [15:0] value;
		begin
			crc_word = crc_byte(crc_byte(current, value[15:8]), value[7:0]);
		end
	endfunction

	task automatic publish_record;
		input [15:0] flags;
		input [15:0] state;
		begin
			@(negedge clk_hdmi);
			copy_state = {state, flags};
			#2 copy_generation = ~copy_generation;
			repeat(4) @(posedge clk_hdmi);
			repeat(6) @(posedge clk_sys);
		end
	endtask

	task automatic strobe_word;
		input [15:0] value;
		input expected_valid;
		input [15:0] expected_data;
		begin
			@(negedge clk_sys);
			io_din = value;
			io_strobe = 1'b1;
			#1;
			if(response_valid !== expected_valid ||
			   (expected_valid && response_data !== expected_data)) begin
				$display("FAIL: strobe=%h valid=%b data=%h expected_valid=%b expected_data=%h",
					value, response_valid, response_data, expected_valid, expected_data);
				$fatal(1);
			end
			@(posedge clk_sys);
			@(negedge clk_sys);
			io_strobe = 1'b0;
		end
	endtask

	task automatic end_command;
		begin
			@(negedge clk_sys);
			io_uio = 1'b0;
			@(posedge clk_sys);
			@(negedge clk_sys);
		end
	endtask

	task automatic read_record;
		integer word_index;
		begin
			io_uio = 1'b1;
			strobe_word(16'h0067, 1'b1, 16'h4d57);
			for(word_index = 0; word_index < 4; word_index = word_index + 1) begin
				@(negedge clk_sys);
				io_din = 16'd0;
				io_strobe = 1'b1;
				#1;
				if(!response_valid) $fatal(1, "missing response word %0d", word_index);
				record[word_index] = response_data;
				@(posedge clk_sys);
				@(negedge clk_sys);
				io_strobe = 1'b0;
			end
			strobe_word(16'd0, 1'b0, 16'd0);
			end_command();
			expected_crc = 16'hffff;
			expected_crc = crc_word(expected_crc, 16'h0067);
			expected_crc = crc_word(expected_crc, 16'h0006);
			expected_crc = crc_word(expected_crc, 16'd3);
			for(word_index = 0; word_index < 3; word_index = word_index + 1)
				expected_crc = crc_word(expected_crc, record[word_index]);
			if(record[0] != 16'd6 || record[3] != expected_crc)
				$fatal(1, "schema/CRC mismatch schema=%h crc=%h expected=%h",
					record[0], record[3], expected_crc);
		end
	endtask

	task automatic expect_record;
		input [15:0] flags;
		input [15:0] state;
		begin
			read_record();
			if(record[1] != flags || record[2] != state)
				$fatal(1, "record mismatch flags=%h state=%h expected=%h/%h",
					record[1], record[2], flags, state);
		end
	endtask

	initial begin
		repeat(3) @(posedge clk_sys);
		reset_active = 1'b0;

		// Retired diagnostic commands and all latch-v5 commands are untouched.
		for(command = 8'h60; command <= 8'h66; command = command + 1) begin
			io_uio = 1'b1; strobe_word(command[15:0], 1'b0, 16'd0); end_command();
		end
		for(command = 8'h50; command <= 8'h5f; command = command + 1) begin
			io_uio = 1'b1; strobe_word(command[15:0], 1'b0, 16'd0); end_command();
		end

		// Partial reads are abortable and reset starts with no captured record.
		io_uio = 1'b1;
		strobe_word(16'h0067, 1'b1, 16'h4d57);
		strobe_word(16'd0, 1'b1, 16'h0006);
		end_command();
		expect_record(16'h0000, 16'h0000);

		// Every copy-retirement event maps exactly and independently into the
		// canonical record. All 32 diagnostic bits are dynamic source data.
		publish_record(16'hffff, 16'hffff);
		expect_record(16'hffff, 16'hffff);
		for(index = 1; index < 16; index = index + 1) begin
			publish_record(16'hffff & ~(16'h0001 << index), 16'h1000 | index);
			expect_record(16'hffff & ~(16'h0001 << index), 16'h1000 | index);
		end

		// A command snapshot remains immutable while a new frame arrives.
		publish_record(16'h8a67, 16'hd57a);
		read_record();
		for(index = 0; index < 4; index = index + 1) saved[index] = record[index];
		io_uio = 1'b1;
		strobe_word(16'h0067, 1'b1, 16'h4d57);
		strobe_word(16'd0, 1'b1, saved[0]);
		copy_state = 32'h2222_a155;
		copy_generation = ~copy_generation;
		for(index = 1; index < 4; index = index + 1)
			strobe_word(16'd0, 1'b1, saved[index]);
		strobe_word(16'd0, 1'b0, 16'd0);
		end_command();
		repeat(4) @(posedge clk_hdmi);
		repeat(6) @(posedge clk_sys);
		expect_record(16'ha155, 16'h2222);

		// Reset during a transaction clears transport and response state.
		io_uio = 1'b1;
		strobe_word(16'h0067, 1'b1, 16'h4d57);
		reset_active = 1'b1;
		repeat(2) @(posedge clk_sys);
		reset_active = 1'b0;
		end_command();
		expect_record(16'h0000, 16'h0000);

		$display("PASS: passive scaler copy-retirement responder");
		$finish;
	end
endmodule

`default_nettype wire
