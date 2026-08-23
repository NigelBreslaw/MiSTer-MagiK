// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

`timescale 1ns/1ps
`default_nettype none

// Disposable passive observer at the unmodified ascal output boundary. The
// HDMI-domain half only counts completed-frame activity. The clk_sys half is a
// bundled-data receiver and read-only UIO responder. No observer output is
// connected to scaler, latch, reset, route, PLL, mux, or pixel control logic.
module mister_magik_raw_scaler_diagnostic (
	input  wire        clk_hdmi,
	input  wire        clk_sys,
	input  wire        reset_active,
	input  wire        raw_ce,
	input  wire [23:0] raw_rgb,
	input  wire        raw_de,
	input  wire        raw_hs,
	input  wire        raw_vs,
	input  wire        io_uio,
	input  wire        io_strobe,
	input  wire [15:0] io_din,
	output wire        response_valid,
	output reg  [15:0] response_data
);

`include "mister_magik_video_diagnostics_protocol.svh"

	reg raw_vs_previous = 1'b0;
	reg [3:0] active_count = 4'd0;
	reg [3:0] nonzero_count = 4'd0;
	reg hs_seen = 1'b0;
	reg [3:0] frame_sequence = 4'd0;
	(* preserve *) reg [15:0] source_state = 16'd0;
	reg source_generation = 1'b0;

	(* altera_attribute = "-name SYNCHRONIZER_IDENTIFICATION FORCED" *)
	reg generation_meta = 1'b0;
	(* altera_attribute = "-name SYNCHRONIZER_IDENTIFICATION FORCED_IF_ASYNCHRONOUS" *)
	reg generation_sync = 1'b0;
	reg generation_seen = 1'b0;
	reg capture_pending = 1'b0;

	reg has_command = 1'b0;
	reg command_selected = 1'b0;
	reg [1:0] word_count = 2'd0;
	(* preserve *) reg [15:0] snapshot_state = 16'd0;
	reg [15:0] tx_crc;
	reg [15:0] response_word;

	wire frame_start = raw_ce && raw_vs && !raw_vs_previous;
	wire active_sample = raw_ce && raw_de;
	wire command_start = io_uio && io_strobe && !has_command;
	wire command_data = io_uio && io_strobe && has_command;
	wire selected_start = io_din[7:0] == MAGIK_UIO_GET_RAW_SCALER_STATE;
	wire selected_command = command_selected;

	assign response_valid =
		(command_start && selected_start) ||
		(command_data && selected_command &&
		 (word_count < MAGIK_RAW_SCALER_STATE_WORDS));

	function automatic [3:0] saturating_increment;
		input [3:0] value;
		begin
			saturating_increment = (&value) ? value : value + 1'd1;
		end
	endfunction

	function automatic [15:0] crc_update_byte;
		input [15:0] crc_in;
		input [7:0] byte_in;
		integer bit_index;
		reg [15:0] value;
		begin
			value = crc_in ^ {byte_in, 8'h00};
			for(bit_index = 0; bit_index < 8; bit_index = bit_index + 1)
				value = value[15] ? ((value << 1) ^ 16'h1021) : (value << 1);
			crc_update_byte = value;
		end
	endfunction

	function automatic [15:0] crc_update_word;
		input [15:0] crc_in;
		input [15:0] word_in;
		begin
			crc_update_word =
				crc_update_byte(crc_update_byte(crc_in, word_in[15:8]), word_in[7:0]);
		end
	endfunction

	localparam [15:0] MAGIK_RAW_SCALER_STATE_SCHEMA_CRC =
		crc_update_word(MAGIK_RAW_SCALER_STATE_HEADER_CRC,
			MAGIK_RAW_SCALER_STATE_SCHEMA);

	// Per-frame state: [15:12] heartbeat, [11:8] nonzero active samples,
	// [7:4] active samples, [3] reserved zero, [2] HS observed, [1] CE
	// observed, [0] completed-frame sample valid. Counts saturate at 15 so one
	// flashing pixel remains distinguishable from substantial image activity.
	always @(posedge clk_hdmi or posedge reset_active) begin
		if(reset_active) begin
			raw_vs_previous <= 1'b0;
			active_count <= 4'd0;
			nonzero_count <= 4'd0;
			hs_seen <= 1'b0;
			frame_sequence <= 4'd0;
			source_state <= 16'd0;
			source_generation <= 1'b0;
		end
		else begin
			if(raw_ce) begin
				raw_vs_previous <= raw_vs;
				hs_seen <= hs_seen | raw_hs;
				if(active_sample)
					active_count <= saturating_increment(active_count);
				if(active_sample && (|raw_rgb))
					nonzero_count <= saturating_increment(nonzero_count);
			end

			if(frame_start) begin
				frame_sequence <= frame_sequence + 1'd1;
				source_state <= {
					frame_sequence + 1'd1,
					nonzero_count,
					active_count,
					1'b0,
					hs_seen | raw_hs,
					1'b1,
					1'b1
				};
				source_generation <= ~source_generation;
				active_count <= 4'd0;
				nonzero_count <= 4'd0;
				hs_seen <= 1'b0;
			end
		end
	end

	always @(*) begin
		case(word_count)
			MAGIK_RAW_SCALER_STATE_SCHEMA_WORD:
				response_word = MAGIK_RAW_SCALER_STATE_SCHEMA;
			MAGIK_RAW_SCALER_STATE_STATE_WORD:
				response_word = snapshot_state;
			default: response_word = tx_crc;
		endcase

		response_data = 16'd0;
		if(command_start && selected_start)
			response_data = MAGIK_RAW_SCALER_STATE_MAGIC;
		else if(command_data && selected_command &&
			(word_count < MAGIK_RAW_SCALER_STATE_WORDS))
			response_data = response_word;
	end

	always @(posedge clk_sys) begin
		generation_meta <= source_generation;
		generation_sync <= generation_meta;

		if(reset_active) begin
			generation_seen <= generation_sync;
			capture_pending <= 1'b0;
			snapshot_state <= 16'd0;
		end
		else if(!has_command && generation_sync != generation_seen) begin
			generation_seen <= generation_sync;
			capture_pending <= 1'b1;
		end
		else if(!has_command && capture_pending) begin
			snapshot_state <= source_state;
			capture_pending <= 1'b0;
		end

		if(command_start) begin
			has_command <= 1'b1;
			command_selected <= selected_start;
			word_count <= 2'd0;
			if(selected_start)
				tx_crc <= MAGIK_RAW_SCALER_STATE_SCHEMA_CRC;
		end
		else if(command_data && selected_command &&
			(word_count < MAGIK_RAW_SCALER_STATE_WORDS)) begin
			word_count <= word_count + 1'd1;
			if(word_count == MAGIK_RAW_SCALER_STATE_STATE_WORD)
				tx_crc <= crc_update_word(tx_crc, response_word);
		end

		if(!io_uio && has_command) begin
			has_command <= 1'b0;
			command_selected <= 1'b0;
			word_count <= 2'd0;
		end
	end
endmodule

`default_nettype wire
