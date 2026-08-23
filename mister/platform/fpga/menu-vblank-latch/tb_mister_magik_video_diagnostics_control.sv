// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

`timescale 1ns/1ps

module tb_mister_magik_video_diagnostics_control;
	reg clk_sys = 1'b0;
	reg reset_active = 1'b1;
	reg io_uio = 1'b0;
	reg io_strobe = 1'b0;
	reg [15:0] io_din = 16'd0;
	reg [15:0] source_state = 16'd0;
	reg source_generation = 1'b0;
	wire response_valid;
	wire [15:0] response_data;
	integer command;
	reg [15:0] expected_crc;

	always #5 clk_sys = ~clk_sys;

	mister_magik_scaler_scheduler_diagnostic dut (
		.clk_sys(clk_sys),
		.reset_active(reset_active),
		.io_uio(io_uio),
		.io_strobe(io_strobe),
		.io_din(io_din),
		.source_state(source_state),
		.source_generation(source_generation),
		.response_valid(response_valid),
		.response_data(response_data)
	);

	function automatic [15:0] crc_byte;
		input [15:0] current;
		input [7:0] value;
		integer bit_index;
		reg [15:0] next_crc;
		begin
			next_crc = current ^ {value, 8'h00};
			for(bit_index = 0; bit_index < 8; bit_index = bit_index + 1)
				next_crc = next_crc[15] ? ((next_crc << 1) ^ 16'h1021) : (next_crc << 1);
			crc_byte = next_crc;
		end
	endfunction

	function automatic [15:0] crc_word;
		input [15:0] current;
		input [15:0] value;
		begin
			crc_word = crc_byte(crc_byte(current, value[15:8]), value[7:0]);
		end
	endfunction

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

	initial begin
		repeat(3) @(posedge clk_sys);
		reset_active = 1'b0;

		// Unsupported retired and latch commands must never collide.
		io_uio = 1'b1;
		strobe_word(16'h0066, 1'b0, 16'd0);
		end_command();
		for(command = 8'h57; command <= 8'h5f; command = command + 1) begin
			io_uio = 1'b1;
			strobe_word(command[15:0], 1'b0, 16'd0);
			end_command();
		end

		// A generation toggle is synchronized, then the bundled state is sampled
		// one further clk_sys edge later.
		source_state = 16'ha55b;
		source_generation = 1'b1;
		repeat(6) @(posedge clk_sys);

		expected_crc = 16'hffff;
		expected_crc = crc_word(expected_crc, 16'h0067);
		expected_crc = crc_word(expected_crc, 16'h0001);
		expected_crc = crc_word(expected_crc, 16'h0002);
		expected_crc = crc_word(expected_crc, 16'h0001);
		expected_crc = crc_word(expected_crc, 16'ha55b);

		io_uio = 1'b1;
		strobe_word(16'h0067, 1'b1, 16'h4d57);
		// Change the live source after command start; this transaction must retain
		// the captured state selected above.
		source_state = 16'h8c31;
		source_generation = 1'b0;
		strobe_word(16'd0, 1'b1, 16'h0001);
		strobe_word(16'd0, 1'b1, 16'ha55b);
		strobe_word(16'd0, 1'b1, expected_crc);
		strobe_word(16'd0, 1'b0, 16'd0);
		end_command();

		// Reset invalidates the cached observation without affecting UIO framing.
		reset_active = 1'b1;
		repeat(2) @(posedge clk_sys);
		reset_active = 1'b0;
		io_uio = 1'b1;
		strobe_word(16'h0067, 1'b1, 16'h4d57);
		strobe_word(16'd0, 1'b1, 16'h0001);
		strobe_word(16'd0, 1'b1, 16'h0000);
		end_command();

		$display("PASS: minimal scaler scheduler diagnostic framing and CDC capture");
		$finish;
	end
endmodule
