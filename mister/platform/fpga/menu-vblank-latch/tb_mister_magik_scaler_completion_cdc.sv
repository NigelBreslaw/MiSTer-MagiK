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
	wire completion_pulse;
	wire completion_batch_two_toggle;
	wire completion_starved_frame_toggle;
	wire completion_starved_line_toggle;
	wire [10:0] completion_frame_state;
	wire completion_snapshot_valid;
	wire completion_delta_invalid;
	reg scaler_vs = 1'b0;
	reg scaler_de = 1'b0;
	reg scaler_framebuffer_enabled = 1'b0;
	reg [1:0] scaler_scheduler_state = 2'd0;
	reg [1:0] scaler_copy_state = 2'd0;
	reg [1:0] scaler_read_level = 2'd0;
	reg [1:0] scaler_copy_level = 2'd0;
	reg batch_two_previous = 1'b0;
	integer batch_two_count = 0;
	reg lev_dec = 1'b0;
	integer modeled_copy_level = 0;

	mister_magik_scaler_completion_cdc dut (
		.destination_clk(destination_clk),
		.reset_n(reset_n),
		.source_completion_gray(source_completion_gray),
		.scaler_vs(scaler_vs),
		.scaler_de(scaler_de),
		.scaler_framebuffer_enabled(scaler_framebuffer_enabled),
		.scaler_scheduler_state(scaler_scheduler_state),
		.scaler_copy_state(scaler_copy_state),
		.scaler_read_level(scaler_read_level),
		.scaler_copy_level(scaler_copy_level),
		.completion_pulse(completion_pulse),
		.completion_batch_two_toggle(completion_batch_two_toggle),
		.completion_starved_frame_toggle(completion_starved_frame_toggle),
		.completion_starved_line_toggle(completion_starved_line_toggle),
		.completion_frame_state(completion_frame_state),
		.completion_snapshot_valid(completion_snapshot_valid),
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

	// Faithful model of the restored legacy ascal COPYLEV truth table.
	always @(posedge destination_clk or negedge reset_n) begin
		if(!reset_n)
			modeled_copy_level <= 0;
		else if(lev_dec && !completion_pulse) begin
			if(modeled_copy_level > 0)
				modeled_copy_level <= modeled_copy_level - 1;
		end
		else if(!lev_dec && completion_pulse) begin
			if(modeled_copy_level < 2)
				modeled_copy_level <= modeled_copy_level + 1;
		end
	end

	always @(posedge destination_clk or negedge reset_n) begin
		if(!reset_n) begin
			recovered_completion_count <= 0;
			batch_two_previous <= 1'b0;
			batch_two_count <= 0;
		end
		else begin
			recovered_completion_count <=
				recovered_completion_count + completion_pulse;
			batch_two_previous <= completion_batch_two_toggle;
			if(completion_batch_two_toggle != batch_two_previous)
				batch_two_count <= batch_two_count + 1;
		end
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
		if(batch_two_count != 1)
			$fatal(1, "batch-two evidence count %0d, expected 1", batch_two_count);
		if(completion_delta_invalid)
			$fatal(1, "valid skip-by-two was classified as invalid");
		if(modeled_copy_level != 2)
			$fatal(1, "serialized recovered credits did not fill both copy slots");

		// A copy finishing while a new completion pulse is sampled is net zero,
		// exactly matching the legacy scaler behavior.
		source_completion();
		wait(completion_pulse == 1'b1);
		@(negedge destination_clk);
		lev_dec = 1'b1;
		@(negedge destination_clk);
		lev_dec = 1'b0;
		if(modeled_copy_level != 2)
			$fatal(1, "simultaneous completion/copy did not hold copy level");

		// A single completion while stopped remains an ordinary delta of one.
		destination_clock_enabled = 1'b0;
		source_completion();
		repeat(8) @(posedge source_clk);
		destination_clock_enabled = 1'b1;
		repeat(8) @(posedge destination_clk);
		if(recovered_completion_count != 4)
			$fatal(1, "single stopped-clock completion was not retained");

		// Source is now at binary 0. Advance once with normal sampling, then
		// stop across two completions to exercise modulo wrap 1 -> 3.
		source_completion();
		repeat(8) @(posedge destination_clk);
		destination_clock_enabled = 1'b0;
		source_completion();
		repeat(128) @(posedge source_clk);
		source_completion();
		destination_clock_enabled = 1'b1;
		repeat(10) @(posedge destination_clk);
		if(recovered_completion_count != 7)
			$fatal(1, "modulo-wrap completion recovery failed");

		// When a destination sample occurs between completions, the same pair
		// arrives as two ordinary delta-one observations rather than one batch.
		source_completion();
		repeat(5) @(posedge destination_clk);
		source_completion();
		repeat(8) @(posedge destination_clk);
		if(recovered_completion_count != 9)
			$fatal(1, "intermediate Gray sampling lost a completion");

		// A persistent two-read/no-copy state is counted only after complete
		// native frame and line intervals, while the exported live state is
		// captured coherently on a scaler VS edge.
		scaler_framebuffer_enabled = 1'b1;
		scaler_scheduler_state = 2'd2;
		scaler_copy_state = 2'd0;
		scaler_read_level = 2'd2;
		scaler_copy_level = 2'd0;
		@(negedge destination_clk); scaler_vs = 1'b1;
		@(negedge destination_clk); scaler_vs = 1'b0;
		@(negedge destination_clk); scaler_de = 1'b1;
		@(negedge destination_clk); scaler_de = 1'b0;
		repeat(2) @(negedge destination_clk);
		@(negedge destination_clk); scaler_de = 1'b1;
		@(negedge destination_clk); scaler_de = 1'b0;
		@(negedge destination_clk); scaler_vs = 1'b1;
		@(negedge destination_clk); scaler_vs = 1'b0;
		if(!completion_snapshot_valid ||
		   completion_frame_state != 11'b00_0_00_10_00_10)
			$fatal(1, "native scaler fetch state snapshot mismatch");
		if(completion_starved_frame_toggle != 1'b1 ||
		   completion_starved_line_toggle != 1'b1)
			$fatal(1, "persistent scaler starvation interval was not counted");

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
		if(recovered_completion_count != 0 || completion_pulse != 0)
			$fatal(1, "reset created a phantom completion");

		// Delta three is outside the structural two-outstanding bound. It must
		// latch evidence and never fabricate a completion pulse.
		destination_clock_enabled = 1'b0;
		source_completion();
		source_completion();
		source_completion();
		destination_clock_enabled = 1'b1;
		repeat(8) @(posedge destination_clk);
		if(!completion_delta_invalid || recovered_completion_count != 0)
			$fatal(1, "invalid delta three was not rejected");

		$display("PASS: reproduced parity loss and retained completion credits");
		$finish;
	end
endmodule

`default_nettype wire
