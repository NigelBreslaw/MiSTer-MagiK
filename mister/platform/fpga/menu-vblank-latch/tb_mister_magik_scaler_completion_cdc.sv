// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

`timescale 1ns/1ps
`default_nettype none

// Executable model of the completion transport embedded in ascal.vhd. The
// production structural checker separately pins the corresponding VHDL text.
module ascal_completion_credit_model (
	input  wire       destination_clk,
	input  wire       reset_n,
	input  wire [1:0] source_completion_gray,
	output wire       completion_event,
	output wire       completion_delta_valid
);
	reg [1:0] completion_gray_meta = 2'd0;
	reg [1:0] completion_gray_sync = 2'd0;
	reg [1:0] completion_seen = 2'd0;

	function automatic [1:0] gray_next;
		input [1:0] value;
		begin
			gray_next = {value[0], !value[1]};
		end
	endfunction

	wire delta_one = completion_gray_sync == gray_next(completion_seen);
	wire delta_two = completion_gray_sync == ~completion_seen;
	assign completion_event = delta_one || delta_two;
	assign completion_delta_valid =
		completion_gray_sync == completion_seen || delta_one || delta_two;

	always @(posedge destination_clk or negedge reset_n) begin
		if(!reset_n) begin
			completion_gray_meta <= 2'd0;
			completion_gray_sync <= 2'd0;
			completion_seen <= 2'd0;
		end
		else begin
			completion_gray_meta <= source_completion_gray;
			completion_gray_sync <= completion_gray_meta;
			if(completion_event)
				completion_seen <= gray_next(completion_seen);
		end
	end
endmodule

module tb_mister_magik_scaler_completion_cdc;
	reg source_clk = 1'b0;
	reg destination_clk = 1'b0;
	reg destination_clock_enabled = 1'b1;
	reg reset_n = 1'b0;
	reg legacy_source_toggle = 1'b0;
	reg [1:0] source_completion_gray = 2'd0;
	reg legacy_meta = 1'b0;
	reg legacy_sync = 1'b0;
	reg legacy_sync_previous = 1'b0;
	integer legacy_completion_count = 0;
	integer recovered_completion_count = 0;
	wire completion_event;
	wire completion_delta_valid;
	reg lev_dec = 1'b0;
	integer modeled_copy_level = 0;
	integer start_steps;

	ascal_completion_credit_model dut (
		.destination_clk(destination_clk),
		.reset_n(reset_n),
		.source_completion_gray(source_completion_gray),
		.completion_event(completion_event),
		.completion_delta_valid(completion_delta_valid)
	);

	always #5 source_clk = !source_clk;
	always #3 begin
		if(destination_clock_enabled)
			destination_clk = !destination_clk;
	end

	function automatic [1:0] gray_next;
		input [1:0] value;
		begin
			gray_next = {value[0], !value[1]};
		end
	endfunction

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

	// Exact legacy COPYLEV truth table, with the internal event substituted for
	// the retired one-bit completion pulse.
	always @(posedge destination_clk or negedge reset_n) begin
		if(!reset_n)
			modeled_copy_level <= 0;
		else if(lev_dec && !completion_event)
			modeled_copy_level <= modeled_copy_level - 1;
		else if(!lev_dec && completion_event)
			modeled_copy_level <= modeled_copy_level + 1;
	end

	always @(posedge destination_clk or negedge reset_n) begin
		if(!reset_n)
			recovered_completion_count <= 0;
		else
			recovered_completion_count <= recovered_completion_count + completion_event;
	end

	task automatic source_completion;
		reg [1:0] previous;
		begin
			@(negedge source_clk);
			legacy_source_toggle = !legacy_source_toggle;
			previous = source_completion_gray;
			source_completion_gray = gray_next(source_completion_gray);
			if(!$onehot(previous ^ source_completion_gray))
				$fatal(1, "source Gray ring changed other than one bit");
		end
	endtask

	task automatic reset_model;
		begin
			destination_clock_enabled = 1'b0;
			reset_n = 1'b0;
			source_completion_gray = 2'd0;
			legacy_source_toggle = 1'b0;
			repeat(2) @(posedge source_clk);
			destination_clock_enabled = 1'b1;
			repeat(2) @(posedge destination_clk);
			reset_n = 1'b1;
			repeat(5) @(posedge destination_clk);
		end
	endtask

	task automatic complete_and_retire_one;
		begin
			source_completion();
			wait(completion_event);
			lev_dec = 1'b1;
			@(posedge destination_clk);
			#1;
			lev_dec = 1'b0;
			repeat(3) @(posedge destination_clk);
		end
	endtask

	initial begin
		// Two completions while clk_hdmi is stopped cancel in the legacy
		// crossing. Prove recovery from every Gray starting state, including
		// both modulo wraps.
		for(start_steps = 0; start_steps < 4; start_steps = start_steps + 1) begin
			reset_model();
			repeat(start_steps)
				complete_and_retire_one();
			destination_clock_enabled = 1'b0;
			source_completion();
			repeat(128) @(posedge source_clk);
			source_completion();
			destination_clock_enabled = 1'b1;
			repeat(8) @(posedge destination_clk);
			if(legacy_completion_count != start_steps ||
			   recovered_completion_count != start_steps + 2)
				$fatal(1, "two-credit stopped-clock recovery failed");
			if(modeled_copy_level != 2)
				$fatal(1, "two recovered credits did not fill both copy slots");
		end

		// A simultaneous completed copy and new credit is net zero.
		reset_model();
		destination_clock_enabled = 1'b0;
		source_completion();
		source_completion();
		destination_clock_enabled = 1'b1;
		repeat(8) @(posedge destination_clk);
		source_completion();
		wait(completion_event);
		lev_dec = 1'b1;
		@(posedge destination_clk);
		#1;
		lev_dec = 1'b0;
		if(modeled_copy_level != 2)
			$fatal(1, "simultaneous completion/copy did not hold copy level");

		// Exercise every Gray starting state and both modulo wraps with normal
		// delta-one observations, retiring each credit to preserve occupancy.
		reset_model();
		repeat(7) begin
			complete_and_retire_one();
		end
		if(recovered_completion_count != 7 || modeled_copy_level != 0)
			$fatal(1, "delta-one Gray sequence or wrap lost a completion");

		// Sampling between a pair produces two ordinary delta-one credits.
		complete_and_retire_one();
		complete_and_retire_one();
		if(recovered_completion_count != 9 || modeled_copy_level != 0)
			$fatal(1, "intermediate sampling lost a completion");

		// Reset while the destination is stopped must not create a phantom.
		reset_model();
		if(recovered_completion_count != 0 || completion_event)
			$fatal(1, "reset created a phantom completion");

		// Delta three is outside the two-outstanding invariant and must not be
		// converted into any credit.
		destination_clock_enabled = 1'b0;
		repeat(3) source_completion();
		destination_clock_enabled = 1'b1;
		repeat(8) @(posedge destination_clk);
		if(completion_delta_valid || recovered_completion_count != 0)
			$fatal(1, "invalid delta three fabricated a completion");

		$display("PASS: reproduced parity loss and retained internal completion credits");
		$finish;
	end
endmodule

`default_nettype wire
