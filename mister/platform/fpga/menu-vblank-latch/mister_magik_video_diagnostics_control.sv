// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

`timescale 1ns/1ps
`default_nettype none

// Disposable passive observer at the unmodified ascal RGB boundary. It
// publishes only the most recently completed frame's active-pixel class and
// first active RGB sample. No observer output is connected to scaler, latch,
// reset, route, PLL, mux, framebuffer, final output, or pixel control/data.
module mister_magik_raw_scaler_diagnostic (
	input  wire        clk_hdmi,
	input  wire        clk_sys,
	input  wire        reset_active,
	input  wire [23:0] raw_rgb,
	input  wire        raw_de,
	input  wire        raw_vs,
	input  wire        io_uio,
	input  wire        io_strobe,
	input  wire [15:0] io_din,
	output wire        response_valid,
	output reg  [15:0] response_data
);

`include "mister_magik_video_diagnostics_protocol.svh"

	// Preserve one coherent HDMI-domain boundary stage so the production ascal
	// RGB/control outputs drive only shallow diagnostic flip-flop inputs. All
	// classification logic operates from the consistently delayed bundle.
	(* preserve *) reg [23:0] raw_rgb_staged = 24'd0;
	(* preserve *) reg raw_de_staged = 1'b0;
	(* preserve *) reg raw_vs_staged = 1'b0;
	reg raw_vs_previous = 1'b0;
	reg frame_open = 1'b0;
	reg active_seen = 1'b0;
	reg any_nonblack = 1'b0;
	reg variation_seen = 1'b0;
	reg [23:0] first_active_rgb = 24'd0;

	// Slots 0..2 are response words 1..3. The complete bundle is registered
	// before its generation toggle changes and remains stable for the receiver.
	(* preserve *) reg [47:0] source_state = 48'd0;
	(* preserve *) reg source_generation = 1'b0;

	(* altera_attribute = "-name SYNCHRONIZER_IDENTIFICATION FORCED" *)
	reg generation_meta = 1'b0;
	(* altera_attribute = "-name SYNCHRONIZER_IDENTIFICATION FORCED_IF_ASYNCHRONOUS" *)
	reg generation_sync = 1'b0;
	reg generation_seen = 1'b0;
	reg capture_pending = 1'b0;

	reg has_command = 1'b0;
	reg command_selected = 1'b0;
	reg [2:0] word_count = 3'd0;
	(* preserve *) reg [47:0] snapshot_state = 48'd0;
	reg [15:0] tx_crc = 16'hffff;
	reg [15:0] response_word;

	wire frame_start = raw_vs_staged && !raw_vs_previous;
	wire completed_frame = frame_start && frame_open;
	wire active_sample = raw_de_staged;
	wire command_start = io_uio && io_strobe && !has_command;
	wire command_data = io_uio && io_strobe && has_command;
	wire selected_start = io_din[7:0] == MAGIK_UIO_GET_RAW_SCALER_STATE;
	wire selected_command = command_selected;

	assign response_valid =
		(command_start && selected_start) ||
		(command_data && selected_command &&
		 (word_count < MAGIK_RAW_SCALER_STATE_WORDS));

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

	// RGB is the production ascal {R,G,B} 8:8:8 output. Its exact black value
	// is 24'h000000. The rising VS sample starts the next frame and is excluded
	// from the completed bundle published on the same edge.
	always @(posedge clk_hdmi or posedge reset_active) begin
		if(reset_active) begin
			raw_rgb_staged <= 24'd0;
			raw_de_staged <= 1'b0;
			raw_vs_staged <= 1'b0;
			raw_vs_previous <= 1'b0;
			frame_open <= 1'b0;
			active_seen <= 1'b0;
			any_nonblack <= 1'b0;
			variation_seen <= 1'b0;
			first_active_rgb <= 24'd0;
			source_state <= 48'd0;
			source_generation <= 1'b0;
		end
		else begin
			raw_rgb_staged <= raw_rgb;
			raw_de_staged <= raw_de;
			raw_vs_staged <= raw_vs;
			raw_vs_previous <= raw_vs_staged;

			if(frame_start) begin
				frame_open <= 1'b1;
				active_seen <= active_sample;
				any_nonblack <= active_sample && (raw_rgb_staged != 24'h000000);
				variation_seen <= 1'b0;
				first_active_rgb <= active_sample ? raw_rgb_staged : 24'd0;

				if(completed_frame) begin
					source_state <= {
						{8'd0, first_active_rgb[23:16]},
						first_active_rgb[15:0],
						MAGIK_RAW_SCALER_STATE_FLAG_FRAME_VALID |
						(active_seen ?
							MAGIK_RAW_SCALER_STATE_FLAG_ACTIVE_SEEN : 16'd0) |
						(any_nonblack ?
							MAGIK_RAW_SCALER_STATE_FLAG_ANY_NONBLACK : 16'd0) |
						(variation_seen ?
							MAGIK_RAW_SCALER_STATE_FLAG_VARIATION_SEEN : 16'd0)
					};
					source_generation <= ~source_generation;
				end
			end
			else if(active_sample) begin
				if(!active_seen) begin
					active_seen <= 1'b1;
					first_active_rgb <= raw_rgb_staged;
				end
				else if(raw_rgb_staged != first_active_rgb)
					variation_seen <= 1'b1;

				if(raw_rgb_staged != 24'h000000)
					any_nonblack <= 1'b1;
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

	// Toggle plus stable bundled data. The receiver waits one clk_sys edge
	// after observing a new generation before copying the complete bundle. A
	// command snapshot remains immutable until io_uio is released.
	always @(posedge clk_sys or posedge reset_active) begin
		if(reset_active) begin
			generation_meta <= 1'b0;
			generation_sync <= 1'b0;
			generation_seen <= 1'b0;
			capture_pending <= 1'b0;
			snapshot_state <= 48'd0;
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
					tx_crc <= crc_update_word(tx_crc, response_word);
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
