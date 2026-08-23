`timescale 1ns/1ps
`default_nettype none

module tb_mister_magik_video_diagnostics_control;
	reg clk_hdmi = 1'b0;
	reg clk_sys = 1'b0;
	reg reset_active = 1'b1;
	reg raw_ce = 1'b1;
	reg [23:0] raw_rgb = 24'd0;
	reg raw_de = 1'b0;
	reg raw_hs = 1'b0;
	reg raw_vs = 1'b0;
	reg io_uio = 1'b0;
	reg io_strobe = 1'b0;
	reg [15:0] io_din = 16'd0;
	wire response_valid;
	wire [15:0] response_data;
	reg [15:0] expected_crc;
	integer command;
	integer frame_index;

	always #7 clk_hdmi = ~clk_hdmi;
	always #5 clk_sys = ~clk_sys;

	mister_magik_raw_scaler_diagnostic dut (
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
		input hs;
		input [23:0] rgb;
		begin
			@(negedge clk_hdmi);
			raw_de = de;
			raw_hs = hs;
			raw_rgb = rgb;
			@(posedge clk_hdmi);
		end
	endtask

	task automatic complete_frame;
		input integer active_samples;
		input integer nonzero_samples;
		input hs_present;
		integer index;
		begin
			raw_vs = 1'b0;
			hdmi_sample(1'b0, hs_present, 24'd0);
			for(index = 0; index < active_samples; index = index + 1)
				hdmi_sample(1'b1, 1'b0,
					index < nonzero_samples ? (24'h010101 + index) : 24'd0);
			hdmi_sample(1'b0, 1'b0, 24'd0);
			@(negedge clk_hdmi);
			raw_vs = 1'b1;
			@(posedge clk_hdmi);
			@(negedge clk_hdmi);
			raw_vs = 1'b0;
			repeat(7) @(posedge clk_sys);
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

	task automatic read_state;
		input [15:0] expected_state;
		begin
			expected_crc = 16'hffff;
			expected_crc = crc_word(expected_crc, 16'h0067);
			expected_crc = crc_word(expected_crc, 16'h0002);
			expected_crc = crc_word(expected_crc, 16'h0002);
			expected_crc = crc_word(expected_crc, 16'h0002);
			expected_crc = crc_word(expected_crc, expected_state);
			io_uio = 1'b1;
			strobe_word(16'h0067, 1'b1, 16'h4d57);
			strobe_word(16'd0, 1'b1, 16'h0002);
			strobe_word(16'd0, 1'b1, expected_state);
			strobe_word(16'd0, 1'b1, expected_crc);
			strobe_word(16'd0, 1'b0, 16'd0);
			end_command();
		end
	endtask

	initial begin
		repeat(3) @(posedge clk_sys);
		reset_active = 1'b0;

		// Retired observer and latch commands remain unsupported by this responder.
		for(command = 8'h60; command <= 8'h66; command = command + 1) begin
			io_uio = 1'b1;
			strobe_word(command[15:0], 1'b0, 16'd0);
			end_command();
		end
		for(command = 8'h57; command <= 8'h5f; command = command + 1) begin
			io_uio = 1'b1;
			strobe_word(command[15:0], 1'b0, 16'd0);
			end_command();
		end

		// Healthy, raw-black, missing-DE, and sparse-frame evidence.
		complete_frame(20, 20, 1'b1);
		read_state(16'h1ff7);
		complete_frame(20, 0, 1'b1);
		read_state(16'h20f7);
		complete_frame(0, 0, 1'b1);
		read_state(16'h3007);
		complete_frame(20, 1, 1'b1);
		read_state(16'h41f7);

		// A stopped HDMI clock-enable cannot manufacture a fresh frame heartbeat.
		raw_ce = 1'b0;
		raw_vs = 1'b1;
		repeat(8) @(posedge clk_hdmi);
		raw_vs = 1'b0;
		read_state(16'h41f7);
		raw_ce = 1'b1;

		// Frame sequence is explicitly modulo 16 and does not affect counters.
		for(frame_index = 0; frame_index < 12; frame_index = frame_index + 1)
			complete_frame(16, 16, 1'b1);
		read_state(16'h0ff7);

		// A response is immutable even when a new raw frame completes mid-command.
		expected_crc = 16'hffff;
		expected_crc = crc_word(expected_crc, 16'h0067);
		expected_crc = crc_word(expected_crc, 16'h0002);
		expected_crc = crc_word(expected_crc, 16'h0002);
		expected_crc = crc_word(expected_crc, 16'h0002);
		expected_crc = crc_word(expected_crc, 16'h0ff7);
		io_uio = 1'b1;
		strobe_word(16'h0067, 1'b1, 16'h4d57);
		complete_frame(20, 0, 1'b1);
		strobe_word(16'd0, 1'b1, 16'h0002);
		strobe_word(16'd0, 1'b1, 16'h0ff7);
		strobe_word(16'd0, 1'b1, expected_crc);
		end_command();
		repeat(7) @(posedge clk_sys);
		read_state(16'h10f7);

		// Reset invalidates both the raw frame sample and clk_sys snapshot.
		reset_active = 1'b1;
		repeat(2) @(posedge clk_sys);
		reset_active = 1'b0;
		read_state(16'h0000);

		$display("PASS: minimal raw scaler boundary diagnostic framing and activity");
		$finish;
	end
endmodule

`default_nettype wire
