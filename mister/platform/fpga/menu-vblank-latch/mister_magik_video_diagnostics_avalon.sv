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
	input  wire       scaler_vs,
	input  wire       scaler_framebuffer_enabled,
	input  wire [1:0] scaler_scheduler_state,
	input  wire [1:0] scaler_copy_state,
	input  wire [1:0] scaler_read_level,
	input  wire [1:0] scaler_copy_level,
	output reg        completion_pulse = 1'b0,
	output reg        completion_batch_two_toggle = 1'b0,
	output reg        completion_starved_frame_toggle = 1'b0,
	output reg        completion_snapshot_valid = 1'b0,
	output reg        completion_delta_invalid = 1'b0
);
	(* altera_attribute = "-name SYNCHRONIZER_IDENTIFICATION FORCED" *)
	reg destination_reset_meta = 1'b0;
	(* altera_attribute = "-name SYNCHRONIZER_IDENTIFICATION FORCED_IF_ASYNCHRONOUS" *)
	reg destination_reset_sync = 1'b0;
	(* altera_attribute = "-name SYNCHRONIZER_IDENTIFICATION FORCED" *)
	reg [1:0] completion_gray_meta = 2'd0;
	(* altera_attribute = "-name SYNCHRONIZER_IDENTIFICATION FORCED_IF_ASYNCHRONOUS" *)
	reg [1:0] completion_gray_sync = 2'd0;
	reg [1:0] completion_seen_binary = 2'd0;
	reg completion_pending = 1'b0;
	reg scaler_vs_previous = 1'b0;
	reg completion_frame_armed = 1'b0;
	reg completion_frame_starved = 1'b0;
	wire destination_reset_n = destination_reset_sync;

	always @(posedge destination_clk or negedge reset_n) begin
		if(!reset_n) begin
			destination_reset_meta <= 1'b0;
			destination_reset_sync <= 1'b0;
		end
		else begin
			destination_reset_meta <= 1'b1;
			destination_reset_sync <= destination_reset_meta;
		end
	end

	function automatic [1:0] gray_to_binary;
		input [1:0] gray;
		begin
			gray_to_binary = {gray[1], gray[1] ^ gray[0]};
		end
	endfunction

	wire [1:0] synchronized_completion_binary =
		gray_to_binary(completion_gray_sync);
	wire [1:0] completion_delta =
		synchronized_completion_binary - completion_seen_binary;
	wire completion_delta_valid = completion_delta != 2'd3;
	wire scaler_vs_rise = scaler_vs && !scaler_vs_previous;
	wire completion_starved_now =
		scaler_framebuffer_enabled &&
		(scaler_scheduler_state == 2'd2) &&
		(scaler_copy_state == 2'd0) &&
		(scaler_read_level == 2'd2) &&
		(scaler_copy_level == 2'd0) &&
		!completion_pending && !completion_pulse;
	wire completion_frame_starved_now =
		completion_frame_starved && completion_starved_now;

	always @(posedge destination_clk or negedge destination_reset_n) begin
		if(!destination_reset_n) begin
			completion_gray_meta <= 2'd0;
			completion_gray_sync <= 2'd0;
			completion_seen_binary <= 2'd0;
			completion_pending <= 1'b0;
			completion_pulse <= 1'b0;
			completion_batch_two_toggle <= 1'b0;
			completion_starved_frame_toggle <= 1'b0;
			completion_snapshot_valid <= 1'b0;
			completion_delta_invalid <= 1'b0;
			scaler_vs_previous <= 1'b0;
			completion_frame_armed <= 1'b0;
			completion_frame_starved <= 1'b0;
		end
		else begin
			completion_gray_meta <= source_completion_gray;
			completion_gray_sync <= completion_gray_meta;
			scaler_vs_previous <= scaler_vs;
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

			if(scaler_vs_rise) begin
				completion_snapshot_valid <= 1'b1;
				if(completion_frame_armed && completion_frame_starved_now)
					completion_starved_frame_toggle <=
						!completion_starved_frame_toggle;
				completion_frame_armed <= 1'b1;
				completion_frame_starved <= completion_starved_now;
			end
			else if(completion_frame_armed)
				completion_frame_starved <= completion_frame_starved_now;
		end
	end
endmodule

`default_nettype wire
