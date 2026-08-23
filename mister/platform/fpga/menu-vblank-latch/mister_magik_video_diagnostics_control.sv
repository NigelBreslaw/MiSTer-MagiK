// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

`timescale 1ns/1ps
`default_nettype none

// Passive bundled-data receiver and read-only UIO responder for the scaler
// scheduler state. The source holds state stable before toggling generation;
// this module waits for the synchronized toggle and one further clk_sys edge
// before sampling the bus. Its outputs feed only sys_top's UIO response mux.
module mister_magik_scaler_scheduler_diagnostic (
	input  wire        clk_sys,
	input  wire        reset_active,
	input  wire        io_uio,
	input  wire        io_strobe,
	input  wire [15:0] io_din,
	input  wire [15:0] source_state,
	input  wire        source_generation,
	output wire        response_valid,
	output reg  [15:0] response_data
);

`include "mister_magik_video_diagnostics_protocol.svh"

	(* altera_attribute = "-name SYNCHRONIZER_IDENTIFICATION FORCED" *)
	reg generation_meta = 1'b0;
	(* altera_attribute = "-name SYNCHRONIZER_IDENTIFICATION FORCED_IF_ASYNCHRONOUS" *)
	reg generation_sync = 1'b0;
	reg generation_seen = 1'b0;
	reg capture_pending = 1'b0;

	reg has_command = 1'b0;
	reg command_selected = 1'b0;
	reg [1:0] word_count = 2'd0;
	reg [15:0] snapshot_state = 16'd0;
	reg [15:0] response_word;

	wire command_start = io_uio && io_strobe && !has_command;
	wire command_data = io_uio && io_strobe && has_command;
	wire selected_start = io_din[7:0] == MAGIK_UIO_GET_SCALER_SCHEDULER_STATE;
	wire selected_command = command_selected;

	assign response_valid =
		(command_start && selected_start) ||
		(command_data && selected_command &&
		 (word_count < MAGIK_SCALER_SCHEDULER_STATE_WORDS));

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

	wire [15:0] snapshot_crc = crc_update_word(
		crc_update_word(MAGIK_SCALER_SCHEDULER_STATE_HEADER_CRC,
			MAGIK_SCALER_SCHEDULER_STATE_SCHEMA), snapshot_state);

	function automatic [15:0] crc_update_word;
		input [15:0] crc_in;
		input [15:0] word_in;
		begin
			crc_update_word =
				crc_update_byte(crc_update_byte(crc_in, word_in[15:8]), word_in[7:0]);
		end
	endfunction

	always @(*) begin
		case(word_count)
			MAGIK_SCALER_SCHEDULER_STATE_SCHEMA_WORD:
				response_word = MAGIK_SCALER_SCHEDULER_STATE_SCHEMA;
			MAGIK_SCALER_SCHEDULER_STATE_STATE_WORD:
				response_word = snapshot_state;
			default: response_word = snapshot_crc;
		endcase

		response_data = 16'd0;
		if(command_start && selected_start)
			response_data = MAGIK_SCALER_SCHEDULER_STATE_MAGIC;
		else if(command_data && selected_command &&
			(word_count < MAGIK_SCALER_SCHEDULER_STATE_WORDS))
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
		end
		else if(command_data && selected_command &&
			(word_count < MAGIK_SCALER_SCHEDULER_STATE_WORDS)) begin
			word_count <= word_count + 1'd1;
		end

		if(!io_uio && has_command) begin
			has_command <= 1'b0;
			command_selected <= 1'b0;
			word_count <= 2'd0;
		end
	end
endmodule

`default_nettype wire
