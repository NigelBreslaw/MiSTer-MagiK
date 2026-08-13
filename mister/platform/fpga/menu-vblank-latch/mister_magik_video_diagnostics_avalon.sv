// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

`timescale 1ns/1ps
`default_nettype none

// Lossless completion-credit transport from the scaler's Avalon clock to its
// output clock. The source value must be a registered two-bit Gray sequence.
// At most two completions may be unseen because ascal itself caps outstanding
// reads at two; modulo delta 3 is therefore an invariant violation, not data.
module mister_magik_scaler_completion_cdc (
	input  wire       destination_clk,
	input  wire       reset_n,
	input  wire [1:0] source_completion_gray,
	output reg        completion_pulse = 1'b0,
	output wire [1:0] consumed_completion_gray,
	output reg        completion_batch_two_toggle = 1'b0,
	output reg        completion_delta_invalid = 1'b0
);
	reg [1:0] destination_reset_release = 2'd0;
	(* altera_attribute = "-name SYNCHRONIZER_IDENTIFICATION FORCED" *)
	reg [1:0] completion_gray_meta = 2'd0;
	(* altera_attribute = "-name SYNCHRONIZER_IDENTIFICATION FORCED_IF_ASYNCHRONOUS" *)
	reg [1:0] completion_gray_sync = 2'd0;
	reg [1:0] completion_seen_binary = 2'd0;
	reg completion_pending = 1'b0;
	wire destination_reset_n = destination_reset_release[1];

	always @(posedge destination_clk or negedge reset_n) begin
		if(!reset_n)
			destination_reset_release <= 2'd0;
		else
			destination_reset_release <= {destination_reset_release[0], 1'b1};
	end

	function automatic [1:0] gray_to_binary;
		input [1:0] gray;
		begin
			gray_to_binary = {gray[1], gray[1] ^ gray[0]};
		end
	endfunction

	function automatic [1:0] binary_to_gray;
		input [1:0] binary;
		begin
			binary_to_gray = binary ^ (binary >> 1);
		end
	endfunction

	wire [1:0] synchronized_completion_binary =
		gray_to_binary(completion_gray_sync);
	wire [1:0] completion_delta =
		synchronized_completion_binary - completion_seen_binary;
	wire completion_delta_valid = completion_delta != 2'd3;
	assign consumed_completion_gray = binary_to_gray(completion_seen_binary);

	always @(posedge destination_clk or negedge destination_reset_n) begin
		if(!destination_reset_n) begin
			completion_gray_meta <= 2'd0;
			completion_gray_sync <= 2'd0;
			completion_seen_binary <= 2'd0;
			completion_pending <= 1'b0;
			completion_pulse <= 1'b0;
			completion_batch_two_toggle <= 1'b0;
			completion_delta_invalid <= 1'b0;
		end
		else begin
			completion_gray_meta <= source_completion_gray;
			completion_gray_sync <= completion_gray_meta;
			completion_pulse <= 1'b0;
			if(completion_pending) begin
				completion_pulse <= 1'b1;
				completion_pending <= 1'b0;
				completion_seen_binary <= completion_seen_binary + 1'd1;
			end
			else if(completion_delta_valid && completion_delta != 2'd0) begin
				completion_pulse <= 1'b1;
				completion_seen_binary <= completion_seen_binary + 1'd1;
				if(completion_delta == 2'd2) begin
					completion_pending <= 1'b1;
					completion_batch_two_toggle <= !completion_batch_two_toggle;
				end
			end
			if(!completion_delta_valid)
				completion_delta_invalid <= 1'b1;
		end
	end
endmodule

`default_nettype wire
