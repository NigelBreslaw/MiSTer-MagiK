// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

`timescale 1ns/1ps
`default_nettype none

module tb_mister_magik_video_diagnostics_control;
	reg clk_hdmi = 1'b0;
	reg clk_sys = 1'b0;
	reg reset_active = 1'b1;
	reg [23:0] raw_rgb = 24'd0;
	reg raw_de = 1'b0;
	reg raw_vs = 1'b0;
	reg io_uio = 1'b0;
	reg io_strobe = 1'b0;
	reg [15:0] io_din = 16'd0;
	wire response_valid;
	wire [15:0] response_data;
	reg [15:0] record [0:4];
	reg [15:0] saved [0:4];
	reg [15:0] expected_crc;
	integer command;
	integer index;

	always #7 clk_hdmi = ~clk_hdmi;
	always #5 clk_sys = ~clk_sys;

	`include "mister_magik_video_diagnostics_protocol.svh"

	mister_magik_raw_scaler_diagnostic dut (
		.clk_hdmi(clk_hdmi), .clk_sys(clk_sys), .reset_active(reset_active),
		.raw_rgb(raw_rgb), .raw_de(raw_de), .raw_vs(raw_vs),
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

	task automatic hdmi_sample;
		input de;
		input vs;
		input [23:0] rgb;
		begin
			@(negedge clk_hdmi);
			raw_de = de;
			raw_vs = vs;
			raw_rgb = rgb;
			@(posedge clk_hdmi);
		end
	endtask

	task automatic frame_boundary;
		begin
			hdmi_sample(1'b0, 1'b1, 24'd0);
			hdmi_sample(1'b0, 1'b0, 24'd0);
		end
	endtask

	// 0 varied, 1 all black, 2 constant nonblack, 3 varied with a different
	// first sample, and 4 empty/no-DE.
	task automatic complete_frame;
		input integer pattern;
		begin
			case(pattern)
				0: begin
					hdmi_sample(1, 0, 24'h112233);
					hdmi_sample(1, 0, 24'h112233);
					hdmi_sample(1, 0, 24'h445566);
				end
				1: begin
					hdmi_sample(1, 0, 24'h000000);
					hdmi_sample(1, 0, 24'h000000);
					hdmi_sample(1, 0, 24'h000000);
				end
				2: begin
					hdmi_sample(1, 0, 24'habcdef);
					hdmi_sample(1, 0, 24'habcdef);
					hdmi_sample(1, 0, 24'habcdef);
				end
				3: begin
					hdmi_sample(1, 0, 24'h010203);
					hdmi_sample(1, 0, 24'h040506);
					hdmi_sample(1, 0, 24'h070809);
				end
				default: begin
					hdmi_sample(0, 0, 24'hffffff);
					hdmi_sample(0, 0, 24'h123456);
				end
			endcase
			frame_boundary();
			repeat(5) @(posedge clk_hdmi);
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
		begin
			io_uio = 1'b1;
			strobe_word(16'h0067, 1'b1, 16'h4d57);
			for(index = 0; index < 5; index = index + 1) begin
				@(negedge clk_sys);
				io_din = 16'd0;
				io_strobe = 1'b1;
				#1;
				if(!response_valid) $fatal(1, "missing response word %0d", index);
				record[index] = response_data;
				@(posedge clk_sys);
				@(negedge clk_sys);
				io_strobe = 1'b0;
			end
			strobe_word(16'd0, 1'b0, 16'd0);
			end_command();
			expected_crc = 16'hffff;
			expected_crc = crc_word(expected_crc, 16'h0067);
			expected_crc = crc_word(expected_crc, 16'h0004);
			expected_crc = crc_word(expected_crc, 16'd4);
			for(index = 0; index < 4; index = index + 1)
				expected_crc = crc_word(expected_crc, record[index]);
			if(record[0] != 16'd4 || record[4] != expected_crc)
				$fatal(1, "schema/CRC mismatch schema=%h crc=%h expected=%h",
					record[0], record[4], expected_crc);
		end
	endtask

	task automatic apply_reset;
		begin
			reset_active = 1'b1;
			repeat(2) @(posedge clk_sys);
			reset_active = 1'b0;
			repeat(3) @(posedge clk_sys);
			frame_boundary();
			repeat(5) @(posedge clk_hdmi);
		end
	endtask

	task automatic expect_record;
		input [15:0] flags;
		input [23:0] first_rgb;
		begin
			read_record();
			if(record[1] != flags || record[2] != first_rgb[15:0] ||
			   record[3] != {8'd0, first_rgb[23:16]})
				$fatal(1, "record mismatch flags=%h rgb=%h/%h expected=%h/%h",
					record[1], record[3], record[2], flags, first_rgb);
		end
	endtask

	initial begin
		repeat(3) @(posedge clk_sys);
		reset_active = 1'b0;

		// Commands retired from this experimental responder and all latch-v5
		// commands remain untouched.
		for(command = 8'h60; command <= 8'h66; command = command + 1) begin
			io_uio = 1'b1; strobe_word(command[15:0], 1'b0, 16'd0); end_command();
		end
		for(command = 8'h50; command <= 8'h5f; command = command + 1) begin
			io_uio = 1'b1; strobe_word(command[15:0], 1'b0, 16'd0); end_command();
		end

		// Partial reads are abortable and reset starts with no completed frame.
		io_uio = 1'b1;
		strobe_word(16'h0067, 1'b1, 16'h4d57);
		strobe_word(16'd0, 1'b1, 16'h0004);
		end_command();
		apply_reset();
		expect_record(16'h0000, 24'h000000);

		// Empty/no-DE frames explicitly invalidate active-frame evidence.
		complete_frame(4);
		expect_record(16'h0001, 24'h000000);

		// A black frame is classifiable from the first completed active frame.
		apply_reset();
		complete_frame(1);
		expect_record(16'h0003, 24'h000000);

		// Constant nonblack and varied frames retain the exact first RGB sample.
		complete_frame(2);
		expect_record(16'h0007, 24'habcdef);
		complete_frame(0);
		expect_record(16'h000f, 24'h112233);
		complete_frame(3);
		expect_record(16'h000f, 24'h010203);

		// Active accumulation cannot leak before the completed-frame boundary.
		for(index = 0; index < 5; index = index + 1) saved[index] = record[index];
		hdmi_sample(1, 0, 24'h999999);
		hdmi_sample(1, 0, 24'haaaaaa);
		repeat(7) @(posedge clk_sys);
		read_record();
		for(index = 0; index < 5; index = index + 1)
			if(record[index] != saved[index]) $fatal(1, "partial frame leaked into export");
		frame_boundary();
		repeat(7) @(posedge clk_sys);
		expect_record(16'h000f, 24'h999999);

		// A transaction remains immutable while a new completed frame arrives.
		for(index = 0; index < 5; index = index + 1) saved[index] = record[index];
		io_uio = 1'b1;
		strobe_word(16'h0067, 1'b1, 16'h4d57);
		strobe_word(16'd0, 1'b1, saved[0]);
		complete_frame(1);
		for(index = 1; index < 5; index = index + 1)
			strobe_word(16'd0, 1'b1, saved[index]);
		strobe_word(16'd0, 1'b0, 16'd0);
		end_command();
		repeat(7) @(posedge clk_sys);
		expect_record(16'h0003, 24'h000000);

		// Reset during accumulation and during a responder transaction is coherent.
		hdmi_sample(1, 0, 24'hffffff);
		reset_active = 1'b1;
		repeat(2) @(posedge clk_sys);
		reset_active = 1'b0;
		frame_boundary();
		complete_frame(2);
		expect_record(16'h0007, 24'habcdef);
		io_uio = 1'b1;
		strobe_word(16'h0067, 1'b1, 16'h4d57);
		reset_active = 1'b1;
		repeat(2) @(posedge clk_sys);
		reset_active = 1'b0;
		end_command();
		expect_record(16'h0000, 24'h000000);

		$display("PASS: minimal raw-scaler RGB state observer and responder");
		$finish;
	end
endmodule

`default_nettype wire
