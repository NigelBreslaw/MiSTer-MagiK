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
	reg legacy_meta = 1'b0;
	reg legacy_sync = 1'b0;
	reg legacy_sync_previous = 1'b0;
	integer legacy_completion_count = 0;

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

	task automatic source_completion;
		begin
			@(negedge source_clk);
			legacy_source_toggle = !legacy_source_toggle;
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

		$display("PASS: reproduced legacy two-completion parity loss");
		$finish;
	end
endmodule

`default_nettype wire
