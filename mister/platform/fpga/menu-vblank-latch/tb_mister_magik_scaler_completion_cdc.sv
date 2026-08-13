// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

`timescale 1ns/1ps
`default_nettype none

module tb_mister_magik_scaler_completion_cdc;
	reg source_clk = 1'b0;
	reg destination_clk = 1'b0;
	reg destination_clock_enabled = 1'b1;
	reg reset_n = 1'b0;
	reg legacy_source_toggle = 1'b0;
	reg [1:0] source_completion_binary = 2'd0;
	reg [1:0] source_completion_gray = 2'd0;
	reg legacy_meta = 1'b0;
	reg legacy_sync = 1'b0;
	reg legacy_sync_previous = 1'b0;
	integer legacy_completion_count = 0;
	integer recovered_completion_count = 0;
	wire [1:0] completion_count;
	wire [1:0] consumed_completion_gray;
	wire [1:0] maximum_completion_batch;
	wire completion_delta_invalid;

	mister_magik_scaler_completion_cdc dut (
		.destination_clk(destination_clk),
		.reset_n(reset_n),
		.source_completion_gray(source_completion_gray),
		.completion_count(completion_count),
		.consumed_completion_gray(consumed_completion_gray),
		.maximum_completion_batch(maximum_completion_batch),
		.completion_delta_invalid(completion_delta_invalid)
	);

	always #5 source_clk = !source_clk;
	always #3 begin
		if(destination_clock_enabled)
			destination_clk = !destination_clk;
	end

	always @(posedge destination_clk or negedge reset_n) begin
		if(!reset_n) begin
			legacy_meta <= 1'b0;
			legacy_sync <= 1'b0;
			legacy_sync_previous <= 1'b0;
			legacy_completion_count <= 0;
		end
		else begin
			legacy_meta <= legacy_source_toggle;
			legacy_sync <= legacy_meta;
			legacy_sync_previous <= legacy_sync;
			if(legacy_sync != legacy_sync_previous)
				legacy_completion_count <= legacy_completion_count + 1;
		end
	end

	always @(posedge destination_clk or negedge reset_n) begin
		if(!reset_n)
			recovered_completion_count <= 0;
		else
			recovered_completion_count <=
				recovered_completion_count + completion_count;
	end

	task automatic source_completion;
		reg [1:0] next_binary;
		begin
			@(negedge source_clk);
			legacy_source_toggle = !legacy_source_toggle;
			next_binary = source_completion_binary + 1'd1;
			source_completion_binary = next_binary;
			source_completion_gray = next_binary ^ (next_binary >> 1);
		end
	endtask

	initial begin
		repeat(2) @(posedge source_clk);
		reset_n = 1'b1;
		repeat(4) @(posedge destination_clk);

		// The scaler permits two outstanding reads. If both blocks complete
		// while clk_hdmi is stopped, the legacy parity token returns to its
		// original value and the destination observes no event after restart.
		destination_clock_enabled = 1'b0;
		source_completion();
		repeat(128) @(posedge source_clk);
		source_completion();
		repeat(8) @(posedge source_clk);
		destination_clock_enabled = 1'b1;
		repeat(8) @(posedge destination_clk);
		if(legacy_completion_count != 0)
			$fatal(1, "legacy parity crossing unexpectedly retained two completions");
		if(recovered_completion_count != 2)
			$fatal(1, "Gray transport recovered %0d completions, expected 2",
				recovered_completion_count);
		if(maximum_completion_batch != 2)
			$fatal(1, "maximum batch %0d, expected skip-by-two", maximum_completion_batch);
		if(completion_delta_invalid)
			$fatal(1, "valid skip-by-two was classified as invalid");

		// A single completion while stopped remains an ordinary delta of one.
		destination_clock_enabled = 1'b0;
		source_completion();
		repeat(8) @(posedge source_clk);
		destination_clock_enabled = 1'b1;
		repeat(8) @(posedge destination_clk);
		if(recovered_completion_count != 3)
			$fatal(1, "single stopped-clock completion was not retained");

		// Reset both domains while the destination clock is stopped. No stale
		// source sequence may emerge as a phantom completion on restart.
		destination_clock_enabled = 1'b0;
		reset_n = 1'b0;
		source_completion_binary = 2'd0;
		source_completion_gray = 2'd0;
		legacy_source_toggle = 1'b0;
		repeat(2) @(posedge source_clk);
		destination_clock_enabled = 1'b1;
		repeat(2) @(posedge destination_clk);
		reset_n = 1'b1;
		repeat(6) @(posedge destination_clk);
		if(recovered_completion_count != 0 || completion_count != 0)
			$fatal(1, "reset created a phantom completion");

		$display("PASS: reproduced parity loss and retained completion credits");
		$finish;
	end
endmodule

`default_nettype wire
