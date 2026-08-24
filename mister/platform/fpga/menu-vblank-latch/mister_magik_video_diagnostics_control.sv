// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

`timescale 1ns/1ps
`default_nettype none

// Disposable passive observer at the unmodified ascal control boundary. It
// retains the first completed-frame control-timing mismatch after a stable
// three-frame baseline. No observer output is connected to scaler, latch,
// reset, route, PLL, mux, framebuffer, or pixel control/data logic.
module mister_magik_raw_scaler_diagnostic (
	input  wire        clk_hdmi,
	input  wire        clk_sys,
	input  wire        reset_active,
	input  wire        raw_ce,
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
	reg frame_open = 1'b0;
	reg ce_seen = 1'b0;
	reg hs_seen = 1'b0;
	reg vs_seen = 1'b0;
	reg de_seen = 1'b0;
	reg [15:0] control_crc = 16'hffff;

	reg candidate_valid = 1'b0;
	reg [1:0] candidate_streak = 2'd0;

	// Slots 0..2 are response words 1..3. Before publication the baseline slot
	// holds the current candidate CRC; no generation toggle exposes it.
	// baseline/first-bad storage and remains stable between publications.
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

	wire frame_start = raw_ce && raw_vs && !raw_vs_previous;
	wire completed_frame = frame_start && frame_open;
	wire completed_nonempty = ce_seen && hs_seen && vs_seen && de_seen;
	wire candidate_matches = source_state[31:16] == control_crc;
	wire baseline_matches = source_state[31:16] == control_crc;
	wire baseline_valid =
		(source_state[15:0] & MAGIK_RAW_SCALER_STATE_FLAG_BASELINE_VALID) != 0;
	wire mismatch_latched =
		(source_state[15:0] & MAGIK_RAW_SCALER_STATE_FLAG_MISMATCH_LATCHED) != 0;
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

	function automatic [15:0] control_crc_update;
		input [15:0] crc_in;
		input [3:0] control_sample;
		integer bit_index;
		reg [15:0] value;
		begin
			value = crc_in ^ {control_sample, 12'h000};
			for(bit_index = 0; bit_index < 4; bit_index = bit_index + 1)
				value = value[15] ? ((value << 1) ^ 16'h1021) : (value << 1);
			control_crc_update = value;
		end
	endfunction

	localparam [15:0] MAGIK_RAW_SCALER_STATE_SCHEMA_CRC =
		crc_update_word(MAGIK_RAW_SCALER_STATE_HEADER_CRC,
			MAGIK_RAW_SCALER_STATE_SCHEMA);

	// Fingerprint the exact ordered raw control waveform. The rising
	// VS sample begins the next frame and is intentionally excluded from the
	// tuple being completed on the same edge.
	always @(posedge clk_hdmi or posedge reset_active) begin
		if(reset_active) begin
			raw_vs_previous <= 1'b0;
			frame_open <= 1'b0;
			ce_seen <= 1'b0;
			hs_seen <= 1'b0;
			vs_seen <= 1'b0;
			de_seen <= 1'b0;
			control_crc <= 16'hffff;
			candidate_valid <= 1'b0;
			candidate_streak <= 2'd0;
			source_state <= 48'd0;
			source_generation <= 1'b0;
		end
		else begin
			raw_vs_previous <= raw_vs;

			if(frame_start) begin
				frame_open <= 1'b1;
				control_crc <= control_crc_update(16'hffff,
					{raw_ce, raw_de, raw_hs, raw_vs});
				ce_seen <= raw_ce;
				hs_seen <= raw_ce && raw_hs;
				vs_seen <= raw_ce && raw_vs;
				de_seen <= raw_ce && raw_de;

				if(completed_frame) begin
					if(!baseline_valid) begin
						if(!completed_nonempty) begin
							candidate_valid <= 1'b0;
							candidate_streak <= 2'd0;
						end
						else if(candidate_valid && candidate_matches) begin
							if(candidate_streak == 2'd2) begin
								source_state <= {
									16'd0, control_crc,
									MAGIK_RAW_SCALER_STATE_FLAG_SAMPLE_VALID |
									MAGIK_RAW_SCALER_STATE_FLAG_SAMPLE_NONEMPTY |
									MAGIK_RAW_SCALER_STATE_FLAG_BASELINE_VALID
								};
								source_generation <= ~source_generation;
								candidate_valid <= 1'b0;
								candidate_streak <= 2'd0;
							end
							else
								candidate_streak <= candidate_streak + 1'd1;
						end
						else begin
							candidate_valid <= 1'b1;
							candidate_streak <= 2'd1;
							source_state[31:16] <= control_crc;
						end
					end
					else if(!mismatch_latched &&
						(!completed_nonempty || !baseline_matches)) begin
						source_state[15:0] <=
							MAGIK_RAW_SCALER_STATE_FLAG_SAMPLE_VALID |
							MAGIK_RAW_SCALER_STATE_FLAG_BASELINE_VALID |
							MAGIK_RAW_SCALER_STATE_FLAG_MISMATCH_LATCHED |
							(completed_nonempty ?
								MAGIK_RAW_SCALER_STATE_FLAG_SAMPLE_NONEMPTY : 16'd0);
						source_state[47:32] <= control_crc;
						source_generation <= ~source_generation;
					end
				end
			end
			else begin
				control_crc <= control_crc_update(control_crc,
					{raw_ce, raw_de, raw_hs, raw_vs});
				ce_seen <= ce_seen | raw_ce;
				hs_seen <= hs_seen | (raw_ce && raw_hs);
				vs_seen <= vs_seen | (raw_ce && raw_vs);
				de_seen <= de_seen | (raw_ce && raw_de);
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

	// Toggle plus stable bundled data. The source changes only on baseline
	// publication or the first sticky mismatch; a transaction snapshot never
	// changes until io_uio is released.
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
