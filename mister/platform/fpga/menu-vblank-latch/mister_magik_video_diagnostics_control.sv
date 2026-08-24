// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

`timescale 1ns/1ps
`default_nettype none

// Disposable passive observer at the unmodified direct-ascal boundary. The
// first register stage is an explicit observer-only isolation boundary. No
// signal declared in this module is permitted to drive production logic.
module mister_magik_raw_scaler_ordered_frame (
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

	localparam [15:0] SIGNATURE_INITIAL = 16'h56da;
	localparam [15:0] SIGNATURE_POLYNOMIAL = 16'ha001;
	localparam [7:0] TOKEN_PIXEL = 8'h01;
	localparam [7:0] TOKEN_LINE_START = 8'h80;
	localparam [7:0] TOKEN_HS = 8'h40;
	localparam [7:0] TOKEN_LINE_END = 8'ha0;

	// Observer-only isolation. These registers are the sole direct consumers
	// of the production ascal output nets.
	(* preserve *) reg        isolated_ce = 1'b0;
	(* preserve *) reg [23:0] isolated_rgb = 24'd0;
	(* preserve *) reg        isolated_de = 1'b0;
	(* preserve *) reg        isolated_hs = 1'b0;
	(* preserve *) reg        isolated_vs = 1'b0;

	reg previous_de = 1'b0;
	reg previous_vs = 1'b0;
	reg frame_open = 1'b0;
	reg frame_nonempty = 1'b0;
	reg [15:0] frame_signature = SIGNATURE_INITIAL;

	reg [15:0] published_sequence = 16'd0;
	reg [15:0] published_signature = 16'd0;
	(* preserve *) reg source_generation = 1'b0;

	// Words 2..3 in ascending response order. The source state remains stable
	// between completed nonempty frames; generation changes only after every
	// source register is updated on the same clk_hdmi edge. Word 1 is derived
	// from a single destination valid bit instead of storing a 16-bit flag word
	// in both clock domains.
	wire [31:0] source_state = {
		published_signature,
		published_sequence
	};

	(* altera_attribute = "-name SYNCHRONIZER_IDENTIFICATION FORCED" *)
	reg generation_meta = 1'b0;
	(* altera_attribute = "-name SYNCHRONIZER_IDENTIFICATION FORCED_IF_ASYNCHRONOUS" *)
	reg generation_sync = 1'b0;
	reg generation_seen = 1'b0;
	reg capture_pending = 1'b0;

	reg has_command = 1'b0;
	reg command_selected = 1'b0;
	reg [2:0] word_count = 3'd0;
	reg [31:0] snapshot_state = 32'd0;
	reg snapshot_valid = 1'b0;
	reg [15:0] tx_crc = 16'hffff;
	reg [15:0] response_word;

	wire frame_start = isolated_ce && isolated_vs && !previous_vs;
	wire command_start = io_uio && io_strobe && !has_command;
	wire command_data = io_uio && io_strobe && has_command;
	wire selected_start = io_din[7:0] == MAGIK_UIO_GET_RAW_SCALER_STATE;
	wire selected_command = command_selected;

	assign response_valid =
		(command_start && selected_start) ||
		(command_data && selected_command &&
		 (word_count < MAGIK_RAW_SCALER_STATE_WORDS));

	// One reflected Galois step consumes one qualified sample token. This is
	// deliberately one shallow update per clk_hdmi edge, rather than four
	// cascaded byte-CRC transforms. Active RGB and DE/HS line boundaries remain
	// ordered in the signature.
	function automatic [15:0] ordered_signature_update;
		input [15:0] signature_in;
		input [31:0] token_in;
		reg [15:0] mixed;
		begin
			mixed = signature_in ^ token_in[15:0] ^ token_in[31:16];
			ordered_signature_update = (mixed >> 1) ^
				(mixed[0] ? SIGNATURE_POLYNOMIAL : 16'd0);
		end
	endfunction

	function automatic [31:0] pixel_token;
		input [23:0] rgb;
		input line_start;
		input hs;
		begin
			pixel_token = {
				TOKEN_PIXEL |
				(line_start ? TOKEN_LINE_START : 8'd0) |
				(hs ? TOKEN_HS : 8'd0),
				rgb
			};
		end
	endfunction

	function automatic [31:0] line_end_token;
		input hs;
		begin
			line_end_token = {
				TOKEN_LINE_END | (hs ? TOKEN_HS : 8'd0),
				24'd0
			};
		end
	endfunction

	function automatic [15:0] crc16_update_byte;
		input [15:0] crc_in;
		input [7:0] byte_in;
		integer bit_index;
		reg [15:0] value;
		begin
			value = crc_in ^ {byte_in, 8'h00};
			for(bit_index = 0; bit_index < 8; bit_index = bit_index + 1)
				value = value[15] ? ((value << 1) ^ 16'h1021) : (value << 1);
			crc16_update_byte = value;
		end
	endfunction

	function automatic [15:0] crc16_update_word;
		input [15:0] crc_in;
		input [15:0] word_in;
		begin
			crc16_update_word = crc16_update_byte(
				crc16_update_byte(crc_in, word_in[15:8]), word_in[7:0]);
		end
	endfunction

	localparam [15:0] MAGIK_RAW_SCALER_STATE_SCHEMA_CRC =
		crc16_update_word(MAGIK_RAW_SCALER_STATE_HEADER_CRC,
			MAGIK_RAW_SCALER_STATE_SCHEMA);

	// The observer consumes the previous cycle's isolated values.
	always @(posedge clk_hdmi or posedge reset_active) begin : ordered_frame
		reg [15:0] completed_signature;
		if(reset_active) begin
			isolated_ce <= 1'b0;
			isolated_rgb <= 24'd0;
			isolated_de <= 1'b0;
			isolated_hs <= 1'b0;
			isolated_vs <= 1'b0;
			previous_de <= 1'b0;
			previous_vs <= 1'b0;
			frame_open <= 1'b0;
			frame_nonempty <= 1'b0;
			frame_signature <= SIGNATURE_INITIAL;
			published_sequence <= 16'd0;
			published_signature <= 16'd0;
			source_generation <= 1'b0;
		end
		else begin
			isolated_ce <= raw_ce;
			isolated_rgb <= raw_rgb;
			isolated_de <= raw_de;
			isolated_hs <= raw_hs;
			isolated_vs <= raw_vs;

			if(isolated_ce) begin
				previous_de <= isolated_de;
				previous_vs <= isolated_vs;

				if(frame_start) begin
					completed_signature = previous_de ?
						ordered_signature_update(frame_signature,
							line_end_token(isolated_hs)) : frame_signature;
					if(frame_open && frame_nonempty) begin
						published_sequence <= published_sequence + 1'd1;
						published_signature <= completed_signature;
						source_generation <= ~source_generation;
					end
					frame_open <= 1'b1;
					frame_nonempty <= 1'b0;
					frame_signature <= SIGNATURE_INITIAL;
				end
				else if(frame_open && isolated_de) begin
					frame_signature <= ordered_signature_update(frame_signature,
						pixel_token(isolated_rgb, !previous_de, isolated_hs));
					frame_nonempty <= 1'b1;
				end
				else if(frame_open && previous_de)
					frame_signature <= ordered_signature_update(frame_signature,
						line_end_token(isolated_hs));
			end
		end
	end

	always @(*) begin
		if(word_count == MAGIK_RAW_SCALER_STATE_SCHEMA_WORD)
			response_word = MAGIK_RAW_SCALER_STATE_SCHEMA;
		else if(word_count == MAGIK_RAW_SCALER_STATE_FLAGS_WORD)
			response_word = snapshot_valid ?
				MAGIK_RAW_SCALER_STATE_FLAG_FRAME_VALID : 16'd0;
		else if(word_count == MAGIK_RAW_SCALER_STATE_CRC_WORD)
			response_word = tx_crc;
		else
			response_word = snapshot_state[(word_count - 2'd2) * 16 +: 16];

		response_data = 16'd0;
		if(command_start && selected_start)
			response_data = MAGIK_RAW_SCALER_STATE_MAGIC;
		else if(command_data && selected_command &&
			(word_count < MAGIK_RAW_SCALER_STATE_WORDS))
			response_data = response_word;
	end

	// Stable bundled-data crossing. Wait one clk_sys edge after the synchronized
	// generation change before copying the complete source bundle. A UIO command
	// snapshots that coherent copy and cannot observe later frame updates.
	always @(posedge clk_sys or posedge reset_active) begin
		if(reset_active) begin
			generation_meta <= 1'b0;
			generation_sync <= 1'b0;
			generation_seen <= 1'b0;
			capture_pending <= 1'b0;
			snapshot_state <= 32'd0;
			snapshot_valid <= 1'b0;
			has_command <= 1'b0;
			command_selected <= 1'b0;
			word_count <= 3'd0;
			tx_crc <= 16'hffff;
		end
		else begin
			generation_meta <= source_generation;
			generation_sync <= generation_meta;

			if(!has_command && generation_sync != generation_seen) begin
				generation_seen <= generation_sync;
				capture_pending <= 1'b1;
			end
			else if(!has_command && capture_pending) begin
				snapshot_state <= source_state;
				snapshot_valid <= 1'b1;
				capture_pending <= 1'b0;
			end

			if(command_start) begin
				has_command <= 1'b1;
				command_selected <= selected_start;
				word_count <= 3'd0;
				if(selected_start)
					tx_crc <= MAGIK_RAW_SCALER_STATE_SCHEMA_CRC;
			end
			else if(command_data && selected_command &&
				(word_count < MAGIK_RAW_SCALER_STATE_WORDS)) begin
				word_count <= word_count + 1'd1;
				if(word_count > MAGIK_RAW_SCALER_STATE_SCHEMA_WORD &&
				   word_count < MAGIK_RAW_SCALER_STATE_CRC_WORD)
					tx_crc <= crc16_update_word(tx_crc, response_word);
			end

			if(!io_uio && has_command) begin
				has_command <= 1'b0;
				command_selected <= 1'b0;
				word_count <= 3'd0;
			end
		end
	end

endmodule

`default_nettype wire
