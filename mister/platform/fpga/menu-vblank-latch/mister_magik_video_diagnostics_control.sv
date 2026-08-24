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

	localparam [31:0] CRC32C_INITIAL = 32'hffffffff;

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
	reg [31:0] frame_crc = CRC32C_INITIAL;
	reg [23:0] frame_pixels = 24'd0;
	reg [11:0] frame_lines = 12'd0;

	reg [15:0] published_flags = 16'd0;
	reg [15:0] published_sequence = 16'd0;
	reg [23:0] published_pixels = 24'd0;
	reg [11:0] published_lines = 12'd0;
	reg [3:0] published_variation = 4'd0;
	reg [31:0] published_newest_crc = 32'd0;
	reg [31:0] published_previous_crc = 32'd0;
	reg [31:0] published_oldest_crc = 32'd0;
	reg [7:0] variation_history = 8'd0;
	reg [3:0] comparison_count = 4'd0;
	(* preserve *) reg source_generation = 1'b0;

	// Words 1..11 in ascending response order. The source state remains stable
	// between completed nonempty frames; generation changes only after all of
	// these source registers are updated on the same clk_hdmi edge.
	wire [175:0] source_state = {
		published_oldest_crc[31:16],
		published_oldest_crc[15:0],
		published_previous_crc[31:16],
		published_previous_crc[15:0],
		published_newest_crc[31:16],
		published_newest_crc[15:0],
		published_variation, published_lines,
		8'd0, published_pixels[23:16],
		published_pixels[15:0],
		published_sequence,
		published_flags
	};

	(* altera_attribute = "-name SYNCHRONIZER_IDENTIFICATION FORCED" *)
	reg generation_meta = 1'b0;
	(* altera_attribute = "-name SYNCHRONIZER_IDENTIFICATION FORCED_IF_ASYNCHRONOUS" *)
	reg generation_sync = 1'b0;
	reg generation_seen = 1'b0;
	reg capture_pending = 1'b0;

	reg has_command = 1'b0;
	reg command_selected = 1'b0;
	reg [4:0] word_count = 5'd0;
	(* preserve *) reg [175:0] snapshot_state = 176'd0;
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

	function automatic [31:0] crc32c_update_byte;
		input [31:0] crc_in;
		input [7:0] byte_in;
		integer bit_index;
		reg [31:0] value;
		begin
			value = crc_in ^ byte_in;
			for(bit_index = 0; bit_index < 8; bit_index = bit_index + 1)
				value = value[0] ? ((value >> 1) ^ 32'h82f63b78) :
					(value >> 1);
			crc32c_update_byte = value;
		end
	endfunction

	function automatic [31:0] crc32c_update_pixel;
		input [31:0] crc_in;
		input [23:0] rgb;
		reg [31:0] value;
		begin
			value = crc32c_update_byte(crc_in, 8'h01);
			value = crc32c_update_byte(value, rgb[23:16]);
			value = crc32c_update_byte(value, rgb[15:8]);
			crc32c_update_pixel = crc32c_update_byte(value, rgb[7:0]);
		end
	endfunction

	function automatic [3:0] popcount8;
		input [7:0] value;
		integer index;
		reg [3:0] count;
		begin
			count = 4'd0;
			for(index = 0; index < 8; index = index + 1)
				count = count + value[index];
			popcount8 = count;
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

	// The observer consumes the previous cycle's isolated values. Variables
	// make delimiter ordering explicit when multiple tokens share one sample.
	always @(posedge clk_hdmi or posedge reset_active) begin : ordered_frame
		reg [31:0] crc_value;
		reg [31:0] completed_crc;
		reg [7:0] next_history;
		reg changed;
		reg [3:0] next_variation;
		if(reset_active) begin
			isolated_ce <= 1'b0;
			isolated_rgb <= 24'd0;
			isolated_de <= 1'b0;
			isolated_hs <= 1'b0;
			isolated_vs <= 1'b0;
			previous_de <= 1'b0;
			previous_vs <= 1'b0;
			frame_open <= 1'b0;
			frame_crc <= CRC32C_INITIAL;
			frame_pixels <= 24'd0;
			frame_lines <= 12'd0;
			published_flags <= 16'd0;
			published_sequence <= 16'd0;
			published_pixels <= 24'd0;
			published_lines <= 12'd0;
			published_variation <= 4'd0;
			published_newest_crc <= 32'd0;
			published_previous_crc <= 32'd0;
			published_oldest_crc <= 32'd0;
			variation_history <= 8'd0;
			comparison_count <= 4'd0;
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
					if(frame_open && frame_pixels != 24'd0) begin
						crc_value = frame_crc;
						if(previous_de)
							crc_value = crc32c_update_byte(crc_value,
								8'ha2 | {7'd0, isolated_hs});
						completed_crc = crc32c_update_byte(crc_value, 8'hf1) ^
							32'hffffffff;
						changed = published_flags[0] &&
							(completed_crc != published_newest_crc ||
							 frame_pixels != published_pixels ||
							 frame_lines != published_lines);
						next_history = {variation_history[6:0], changed};
						next_variation = popcount8(next_history);

						published_sequence <= published_sequence + 1'd1;
						published_pixels <= frame_pixels;
						published_lines <= frame_lines;
						published_variation <= next_variation;
						published_oldest_crc <= published_previous_crc;
						published_previous_crc <= published_newest_crc;
						published_newest_crc <= completed_crc;
						variation_history <= next_history;
						if(comparison_count < 4'd8)
							comparison_count <= comparison_count + 1'd1;
						published_flags <=
							MAGIK_RAW_SCALER_STATE_FLAG_FRAME_VALID |
							MAGIK_RAW_SCALER_STATE_FLAG_NONEMPTY |
							((comparison_count >= 4'd7) ?
								MAGIK_RAW_SCALER_STATE_FLAG_VARIATION_WINDOW_FULL :
								16'd0) |
							((next_variation == 4'd8) ?
								MAGIK_RAW_SCALER_STATE_FLAG_VARIATION_SATURATED :
								16'd0);
						source_generation <= ~source_generation;
					end

					frame_open <= 1'b1;
					frame_crc <= crc32c_update_byte(CRC32C_INITIAL, 8'hf0);
					frame_pixels <= 24'd0;
					frame_lines <= 12'd0;
				end
				else if(frame_open) begin
					crc_value = frame_crc;
					if(isolated_de && !previous_de) begin
						crc_value = crc32c_update_byte(crc_value,
							8'ha0 | {7'd0, isolated_hs});
						if(frame_lines != 12'hfff)
							frame_lines <= frame_lines + 1'd1;
					end
					if(isolated_de) begin
						crc_value = crc32c_update_pixel(crc_value, isolated_rgb);
						if(frame_pixels != 24'hffffff)
							frame_pixels <= frame_pixels + 1'd1;
					end
					else if(previous_de)
						crc_value = crc32c_update_byte(crc_value,
							8'ha2 | {7'd0, isolated_hs});
					frame_crc <= crc_value;
				end
			end
		end
	end

	always @(*) begin
		if(word_count == MAGIK_RAW_SCALER_STATE_SCHEMA_WORD)
			response_word = MAGIK_RAW_SCALER_STATE_SCHEMA;
		else if(word_count == MAGIK_RAW_SCALER_STATE_CRC_WORD)
			response_word = tx_crc;
		else
			response_word = snapshot_state[(word_count - 1'd1) * 16 +: 16];

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
			snapshot_state <= 176'd0;
			has_command <= 1'b0;
			command_selected <= 1'b0;
			word_count <= 5'd0;
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
				capture_pending <= 1'b0;
			end

			if(command_start) begin
				has_command <= 1'b1;
				command_selected <= selected_start;
				word_count <= 5'd0;
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
				word_count <= 5'd0;
			end
		end
	end

endmodule

`default_nettype wire
