// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

`timescale 1ns/1ps
`default_nettype none

// Cycle-accurate executable model of the request/pending/ack transport embedded
// in ascal.vhd. The GHDL gate exercises the exact VHDL transition function.
module ascal_completion_queue_model (
	input  wire source_clk,
	input  wire destination_clk,
	input  wire reset_n,
	input  wire completion,
	output reg  request_toggle,
	output reg  completion_pending,
	output reg  completion_pulse,
	output wire overflow
);
	reg request_meta;
	reg request_sync;
	reg completion_ack_meta;
	reg completion_ack_sync;

	assign overflow = request_toggle != completion_ack_sync &&
		completion_pending && completion;

	always @(posedge source_clk or negedge reset_n) begin
		if(!reset_n) begin
			request_toggle <= 1'b0;
			completion_pending <= 1'b0;
			completion_ack_meta <= 1'b0;
			completion_ack_sync <= 1'b0;
		end
		else begin
			completion_ack_meta <= request_sync;
			completion_ack_sync <= completion_ack_meta;
			if(request_toggle == completion_ack_sync) begin
				if(completion_pending) begin
					request_toggle <= !request_toggle;
					completion_pending <= completion;
				end
				else if(completion)
					request_toggle <= !request_toggle;
			end
			else if(completion && !completion_pending)
				completion_pending <= 1'b1;
		end
	end

	always @(posedge destination_clk or negedge reset_n) begin
		if(!reset_n) begin
			request_meta <= 1'b0;
			request_sync <= 1'b0;
			completion_pulse <= 1'b0;
		end
		else begin
			request_meta <= request_toggle;
			request_sync <= request_meta;
			completion_pulse <= request_meta ^ request_sync;
		end
	end
endmodule

module tb_mister_magik_scaler_completion_cdc;
	reg source_clk = 1'b0;
	reg destination_clk = 1'b0;
	reg destination_clock_enabled = 1'b1;
	reg reset_n = 1'b0;
	reg completion = 1'b0;
	wire request_toggle;
	wire completion_pending;
	wire completion_pulse;
	wire overflow;
	integer produced = 0;
	integer consumed = 0;
	integer phase;
	integer spacing;

	ascal_completion_queue_model dut (
		.source_clk(source_clk),
		.destination_clk(destination_clk),
		.reset_n(reset_n),
		.completion(completion),
		.request_toggle(request_toggle),
		.completion_pending(completion_pending),
		.completion_pulse(completion_pulse),
		.overflow(overflow)
	);

	always #5 source_clk = !source_clk;
	always #3 begin
		if(destination_clock_enabled)
			destination_clk = !destination_clk;
	end

	always @(posedge destination_clk or negedge reset_n) begin
		if(!reset_n)
			consumed <= 0;
		else if(completion_pulse)
			consumed <= consumed + 1;
	end

	task automatic reset_model;
		begin
			reset_n = 1'b0;
			completion = 1'b0;
			destination_clock_enabled = 1'b1;
			produced = 0;
			repeat(3) @(posedge source_clk);
			reset_n = 1'b1;
			repeat(4) @(posedge source_clk);
		end
	endtask

	task automatic produce_completion;
		begin
			@(negedge source_clk);
			completion = 1'b1;
			@(negedge source_clk);
			completion = 1'b0;
			produced = produced + 1;
			#1;
			if(overflow)
				$fatal(1, "legal two-credit schedule overflowed");
		end
	endtask

	task automatic await_conservation;
		integer timeout;
		begin
			timeout = 0;
			while(consumed != produced && timeout < 100) begin
				@(posedge source_clk);
				timeout = timeout + 1;
			end
			if(consumed != produced)
				$fatal(1, "completion conservation timeout produced=%0d consumed=%0d",
					produced, consumed);
		end
	endtask

	initial begin
		// Sweep the destination stop point across both clock phases and vary the
		// true 128-beat-or-greater completion spacing.
		for(phase = 0; phase < 6; phase = phase + 1) begin
			for(spacing = 128; spacing <= 132; spacing = spacing + 2) begin
				reset_model();
				repeat(phase) #1;
				destination_clock_enabled = 1'b0;
				produce_completion();
				repeat(spacing) @(posedge source_clk);
				produce_completion();
				if(!completion_pending)
					$fatal(1, "second completion was not queued");
				destination_clock_enabled = 1'b1;
				await_conservation();
				if(completion_pending)
					$fatal(1, "pending completion did not drain");
			end
		end

		// A completion coincident with acknowledgement forwarding must remain
		// pending and appear as the next destination pulse.
		reset_model();
		destination_clock_enabled = 1'b0;
		produce_completion();
		produce_completion();
		destination_clock_enabled = 1'b1;
		wait(request_toggle == dut.completion_ack_sync && completion_pending);
		produce_completion();
		await_conservation();

		// Common reset at every queued phase clears both domains without a
		// phantom destination pulse.
		reset_model();
		destination_clock_enabled = 1'b0;
		produce_completion();
		produce_completion();
		reset_n = 1'b0;
		repeat(3) @(posedge source_clk);
		destination_clock_enabled = 1'b1;
		reset_n = 1'b1;
		repeat(8) @(posedge destination_clk);
		if(completion_pulse || request_toggle || completion_pending || consumed)
			$fatal(1, "reset left stale completion state");

		$display("PASS: queued one-bit completion request survives stopped destination clock");
		$finish;
	end
endmodule

`default_nettype wire
