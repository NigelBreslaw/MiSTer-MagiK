// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

`timescale 1ns/1ps
`default_nettype none

module tb_mister_magik_video_diagnostics_control;
	reg clk_hdmi = 1'b0;
	reg clk_sys = 1'b0;
	reg reset_active = 1'b1;
	reg raw_ce = 1'b1;
	reg raw_de = 1'b0;
	reg raw_hs = 1'b0;
	reg raw_vs = 1'b0;
	reg io_uio = 1'b0;
	reg io_strobe = 1'b0;
	reg [15:0] io_din = 16'd0;
	wire response_valid;
	wire [15:0] response_data;
	reg [15:0] record [0:5];
	reg [15:0] saved_bad [0:5];
	reg [15:0] expected_crc;
	integer command;
	integer index;

	always #7 clk_hdmi = ~clk_hdmi;
	always #5 clk_sys = ~clk_sys;

	`include "mister_magik_video_diagnostics_protocol.svh"

	mister_magik_raw_scaler_diagnostic dut (
		.clk_hdmi(clk_hdmi), .clk_sys(clk_sys), .reset_active(reset_active),
		.raw_ce(raw_ce), .raw_de(raw_de), .raw_hs(raw_hs), .raw_vs(raw_vs),
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
		input ce;
		input de;
		input hs;
		input vs;
		begin
			@(negedge clk_hdmi);
			raw_ce = ce;
			raw_de = de;
			raw_hs = hs;
			raw_vs = vs;
			@(posedge clk_hdmi);
		end
	endtask

	task automatic frame_boundary;
		begin
			hdmi_sample(1'b1, 1'b0, 1'b0, 1'b1);
			hdmi_sample(1'b1, 1'b0, 1'b0, 1'b0);
		end
	endtask

	// Patterns 0 and 1 have identical aggregate counts but different ordering.
	// Patterns 2, 3, and 4 change HS, DE-start, and active counts respectively.
	task automatic complete_pattern;
		input integer pattern;
		begin
			case(pattern)
				0: begin
					hdmi_sample(1, 0, 1, 0); hdmi_sample(1, 0, 0, 0);
					hdmi_sample(1, 1, 0, 0); hdmi_sample(1, 1, 0, 0);
					hdmi_sample(1, 1, 0, 0); hdmi_sample(1, 0, 0, 0);
				end
				1: begin
					hdmi_sample(1, 1, 0, 0); hdmi_sample(1, 1, 0, 0);
					hdmi_sample(1, 1, 0, 0); hdmi_sample(1, 0, 0, 0);
					hdmi_sample(1, 0, 1, 0); hdmi_sample(1, 0, 0, 0);
				end
				2: begin
					hdmi_sample(1, 0, 1, 0); hdmi_sample(1, 0, 0, 0);
					hdmi_sample(1, 0, 1, 0); hdmi_sample(1, 0, 0, 0);
					hdmi_sample(1, 1, 0, 0); hdmi_sample(1, 1, 0, 0);
					hdmi_sample(1, 1, 0, 0); hdmi_sample(1, 0, 0, 0);
				end
				3: begin
					hdmi_sample(1, 0, 1, 0); hdmi_sample(1, 0, 0, 0);
					hdmi_sample(1, 1, 0, 0); hdmi_sample(1, 0, 0, 0);
					hdmi_sample(1, 1, 0, 0); hdmi_sample(1, 1, 0, 0);
					hdmi_sample(1, 0, 0, 0);
				end
				4: begin
					hdmi_sample(1, 0, 1, 0); hdmi_sample(1, 0, 0, 0);
					hdmi_sample(1, 1, 0, 0); hdmi_sample(1, 1, 0, 0);
					hdmi_sample(1, 1, 0, 0); hdmi_sample(1, 1, 0, 0);
					hdmi_sample(1, 0, 0, 0);
				end
				default: begin
					hdmi_sample(1, 0, 1, 0); hdmi_sample(1, 0, 0, 0);
					hdmi_sample(1, 0, 0, 0); hdmi_sample(1, 0, 0, 0);
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
			for(index = 0; index < 6; index = index + 1) begin
				@(negedge clk_sys);
				io_din = 16'd0;
				io_strobe = 1'b1;
				#1;
				if(!response_valid) begin
					$display("FAIL: missing response word %0d", index);
					$fatal(1);
				end
				record[index] = response_data;
				@(posedge clk_sys);
				@(negedge clk_sys);
				io_strobe = 1'b0;
			end
			strobe_word(16'd0, 1'b0, 16'd0);
			end_command();
			expected_crc = 16'hffff;
			expected_crc = crc_word(expected_crc, 16'h0067);
			expected_crc = crc_word(expected_crc, 16'h0003);
			expected_crc = crc_word(expected_crc, 16'd5);
			for(index = 0; index < 5; index = index + 1)
				expected_crc = crc_word(expected_crc, record[index]);
			if(record[0] != 16'd3 || record[5] != expected_crc) begin
				$display("FAIL: schema/CRC schema=%h crc=%h expected=%h",
					record[0], record[5], expected_crc);
				$fatal(1);
			end
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

	task automatic establish_baseline;
		begin
			apply_reset();
			complete_pattern(0);
			if(dut.source_state[15:0] != 0)
				$fatal(1, "baseline established after one frame");
			complete_pattern(0);
			if(dut.source_state[15:0] != 0)
				$fatal(1, "baseline established after two frames");
			complete_pattern(0);
			read_record();
			if((record[1] & 16'h000f) != 16'h0007)
				$fatal(1, "baseline not valid after exactly three frames: %h", record[1]);
		end
	endtask

	initial begin
		repeat(3) @(posedge clk_sys);
		reset_active = 1'b0;

		// Retired diagnostics and every latch-v5 command are unsupported here.
		for(command = 8'h60; command <= 8'h66; command = command + 1) begin
			io_uio = 1'b1; strobe_word(command[15:0], 1'b0, 16'd0); end_command();
		end
		for(command = 8'h50; command <= 8'h5f; command = command + 1) begin
			io_uio = 1'b1; strobe_word(command[15:0], 1'b0, 16'd0); end_command();
		end
		// An aborted partial read cannot poison the next command.
		io_uio = 1'b1;
		strobe_word(16'h0067, 1'b1, 16'h4d57);
		strobe_word(16'd0, 1'b1, 16'h0003);
		end_command();

		// Changing and empty frames cannot create a baseline.
		frame_boundary();
		complete_pattern(0); complete_pattern(1); complete_pattern(0); complete_pattern(5);
		read_record();
		if(record[1] != 0) $fatal(1, "changing/empty sequence established baseline");

		// Reset during candidate streaks returns to a coherent empty record.
		complete_pattern(0); apply_reset();
		complete_pattern(0); complete_pattern(0); apply_reset();
		read_record();
		if(record[1] != 0) $fatal(1, "candidate reset did not clear observer");

		// Phase/order-only mismatch retains equal counts but a different CRC.
		establish_baseline();
		for(index = 0; index < 6; index = index + 1) saved_bad[index] = record[index];
		// The response stays immutable while the first mismatch arrives.
		io_uio = 1'b1;
		strobe_word(16'h0067, 1'b1, 16'h4d57);
		strobe_word(16'd0, 1'b1, saved_bad[0]);
		complete_pattern(1);
		for(index = 1; index < 6; index = index + 1)
			strobe_word(16'd0, 1'b1, saved_bad[index]);
		strobe_word(16'd0, 1'b0, 16'd0);
		end_command();
		repeat(7) @(posedge clk_sys);
		read_record();
		if((record[1] & 16'h000f) != 16'h000f || record[2] == record[3])
			$fatal(1, "phase-only mismatch evidence wrong");
		for(index = 0; index < 6; index = index + 1) saved_bad[index] = record[index];
		complete_pattern(0); complete_pattern(4);
		read_record();
		for(index = 0; index < 6; index = index + 1)
			if(record[index] != saved_bad[index]) $fatal(1, "first-bad record mutated");

		// Independent HS, DE, and active waveform changes alter the fingerprint.
		apply_reset(); establish_baseline(); complete_pattern(2); read_record();
		if(record[2] == record[3]) $fatal(1, "HS mismatch not retained");
		apply_reset(); establish_baseline(); complete_pattern(3); read_record();
		if(record[2] == record[3]) $fatal(1, "DE mismatch not retained");
		apply_reset(); establish_baseline(); complete_pattern(4); read_record();
		if(record[2] == record[3])
			$fatal(1, "active-count mismatch not retained");

		// Frame sequence wraps coherently and reset clears retained evidence.
		apply_reset(); establish_baseline();
		dut.frame_sequence = 16'hffff;
		complete_pattern(1); read_record();
		if(record[4] != 16'h0000) $fatal(1, "frame sequence did not wrap");
		io_uio = 1'b1;
		strobe_word(16'h0067, 1'b1, 16'h4d57);
		reset_active = 1'b1;
		repeat(2) @(posedge clk_sys);
		reset_active = 1'b0;
		end_command();
		read_record();
		if(record[1] != 0) $fatal(1, "reset did not clear retained mismatch");

		$display("PASS: sticky raw-scaler frame-integrity observer and responder");
		$finish;
	end
endmodule

`default_nettype wire
