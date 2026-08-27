// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

`timescale 1ns/1ps
`default_nettype none

// Disposable passive observer at the external scaler Avalon boundary. It
// reconstructs the accepted two-deep transaction order independently from the
// production scaler and fingerprints accepted addresses together with every
// returned beat. No observer output is permitted to drive production logic.
module mister_magik_scaler_fetch_ordered_frame (
	input  wire         clk_100m,
	input  wire         clk_sys,
	input  wire         reset_active,
	input  wire [27:0]  vbuf_address,
	input  wire [7:0]   vbuf_burstcount,
	input  wire         vbuf_waitrequest,
	input  wire [127:0] vbuf_readdata,
	input  wire         vbuf_readdatavalid,
	input  wire         vbuf_read,
	input  wire         io_uio,
	input  wire         io_strobe,
	input  wire [15:0]  io_din,
	output wire         response_valid,
	output reg  [15:0]  response_data
);

`include "mister_magik_video_diagnostics_protocol.svh"

	// Keep the active schema and flag assignments generated from the canonical
	// JSON contract while preserving schema 10's five-word transport shape.
	localparam [15:0] FETCH_STATE_SCHEMA = MAGIK_RAW_SCALER_STATE_SCHEMA;
	localparam [15:0] SIGNATURE_INITIAL = 16'h56da;
	localparam [15:0] SIGNATURE_POLYNOMIAL = 16'ha001;
	localparam [7:0] REQUIRED_BURSTCOUNT = 8'd128;
	localparam [15:0] TOKEN_DATA = 16'h5a02;
	localparam [15:0] TOKEN_ADDRESS = 16'ha501;

	localparam [6:0] FETCH_FLAG_CAPTURE_VALID =
		MAGIK_RAW_SCALER_STATE_FLAG_CAPTURE_VALID;
	localparam [6:0] FETCH_FLAG_FIFO_OVERFLOW =
		MAGIK_RAW_SCALER_STATE_FLAG_FIFO_OVERFLOW;
	localparam [6:0] FETCH_FLAG_UNEXPECTED_RETURN =
		MAGIK_RAW_SCALER_STATE_FLAG_UNEXPECTED_RETURN;
	localparam [6:0] FETCH_FLAG_BAD_BURSTCOUNT =
		MAGIK_RAW_SCALER_STATE_FLAG_BAD_BURSTCOUNT;
	localparam [6:0] FETCH_FLAG_BAD_RETURN_PHASE =
		MAGIK_RAW_SCALER_STATE_FLAG_BAD_RETURN_PHASE;
	localparam [6:0] FETCH_FLAG_EPOCH_OVERLAP =
		MAGIK_RAW_SCALER_STATE_FLAG_EPOCH_OVERLAP;
	localparam [6:0] FETCH_FLAG_COUNTER_OVERFLOW =
		MAGIK_RAW_SCALER_STATE_FLAG_COUNTER_OVERFLOW;

	// Independent accepted-request FIFO. Only a folded address and its epoch
	// marker are retained; production ascal itself caps outstanding reads at two.
	reg [15:0] fifo_address_token0 = 16'd0;
	reg [15:0] fifo_address_token1 = 16'd0;
	reg        fifo_wrap0 = 1'b0;
	reg        fifo_wrap1 = 1'b0;
	reg [1:0]  fifo_count = 2'd0;
	reg [6:0]  return_phase = 7'd0;

	reg [20:0] previous_address = 21'd0;
	reg        previous_address_valid = 1'b0;
	reg        epoch_armed = 1'b0;
	reg        source_faulted = 1'b0;
	reg [15:0] epoch_signature = SIGNATURE_INITIAL;

	reg [15:0] published_signature = 16'd0;
	reg [6:0]  published_flags = 7'd0;
	(* preserve, dont_replicate *) reg source_generation = 1'b0;

	(* altera_attribute = "-name SYNCHRONIZER_IDENTIFICATION FORCED" *)
	reg generation_meta = 1'b0;
	(* altera_attribute = "-name SYNCHRONIZER_IDENTIFICATION FORCED_IF_ASYNCHRONOUS" *)
	reg generation_sync = 1'b0;
	reg generation_seen = 1'b0;
	reg capture_pending = 1'b0;

	reg [15:0] snapshot_signature = 16'd0;
	reg [15:0] snapshot_sequence = 16'd0;
	reg [6:0] snapshot_flags = 7'd0;
	reg has_command = 1'b0;
	reg command_selected = 1'b0;
	reg [2:0] word_count = 3'd0;
	reg [15:0] tx_crc = 16'hffff;
	reg [15:0] response_word;

	wire accepted = vbuf_read && !vbuf_waitrequest;
	wire returned = vbuf_readdatavalid;
	wire return_has_entry = returned && fifo_count != 2'd0;
	wire return_last = return_has_entry && return_phase == 7'd127;
	wire request_shape_valid = vbuf_burstcount == REQUIRED_BURSTCOUNT;
	wire enqueue = accepted && request_shape_valid &&
		(fifo_count != 2'd2 || return_last);
	wire dequeue = return_last;
	wire accepted_wrap = previous_address_valid &&
		vbuf_address[27:7] < previous_address;
	wire marker_consumed = return_has_entry && return_phase == 7'd0 && fifo_wrap0;
	wire marker_still_pending =
		(fifo_wrap0 && !marker_consumed) || (fifo_count == 2'd2 && fifo_wrap1);

	wire [6:0] fault_event = {
		fifo_count == 2'd3,
		accepted && accepted_wrap && marker_still_pending,
		return_has_entry && fifo_wrap0 && return_phase != 7'd0,
		accepted && vbuf_burstcount != REQUIRED_BURSTCOUNT,
		returned && fifo_count == 2'd0,
		accepted && request_shape_valid && fifo_count == 2'd2 && !return_last,
		1'b0
	};

	wire command_start = io_uio && io_strobe && !has_command;
	wire command_data = io_uio && io_strobe && has_command;
	wire selected_start = io_din[7:0] == MAGIK_UIO_GET_RAW_SCALER_STATE;
	wire selected_command = command_selected;

	assign response_valid =
		(command_start && selected_start) ||
		(command_data && selected_command &&
		 (word_count < MAGIK_RAW_SCALER_STATE_WORDS));

	function automatic [15:0] ordered_signature_update;
		input [15:0] signature_in;
		input [15:0] token_in;
		reg [15:0] mixed;
		begin
			mixed = signature_in ^ token_in;
			ordered_signature_update = (mixed >> 1) ^
				(mixed[0] ? SIGNATURE_POLYNOMIAL : 16'd0);
		end
	endfunction

	// Distinct fixed rotations make the reduction sensitive to 16-bit lane
	// permutations without a byte-serial CRC cone or a 128-bit isolation bank.
	function automatic [15:0] fold_return_data;
		input [127:0] data;
		begin
			fold_return_data =
				data[15:0] ^
				{data[30:16], data[31]} ^
				{data[44:32], data[47:45]} ^
				{data[58:48], data[63:59]} ^
				{data[72:64], data[79:73]} ^
				{data[86:80], data[95:87]} ^
				{data[100:96], data[111:101]} ^
				{data[114:112], data[127:115]};
		end
	endfunction

	function automatic [15:0] fold_address;
		input [27:0] address;
		begin
			fold_address = address[15:0] ^ {4'd0, address[27:16]} ^
				TOKEN_ADDRESS;
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

	localparam [15:0] FETCH_HEADER_CRC = crc16_update_word(
		crc16_update_word(
			crc16_update_word(16'hffff,
				{8'd0, MAGIK_UIO_GET_RAW_SCALER_STATE}),
			FETCH_STATE_SCHEMA),
		MAGIK_RAW_SCALER_STATE_WORDS - 1'd1);
	localparam [15:0] FETCH_SCHEMA_CRC =
		crc16_update_word(FETCH_HEADER_CRC, FETCH_STATE_SCHEMA);

	// The actual accept/return events, not DUT credits, own this scoreboard.
	// A wrap marker reaches the signature only with its accepted transaction's
	// first return, so no asynchronous video-frame marker is required.
	always @(posedge clk_100m or posedge reset_active) begin : fetch_order
		reg [15:0] return_token;
		if(reset_active) begin
			fifo_address_token0 <= 16'd0;
			fifo_address_token1 <= 16'd0;
			fifo_wrap0 <= 1'b0;
			fifo_wrap1 <= 1'b0;
			fifo_count <= 2'd0;
			return_phase <= 7'd0;
			previous_address <= 21'd0;
			previous_address_valid <= 1'b0;
			epoch_armed <= 1'b0;
			source_faulted <= 1'b0;
			epoch_signature <= SIGNATURE_INITIAL;
			published_signature <= 16'd0;
			published_flags <= 7'd0;
			source_generation <= 1'b0;
		end
		else begin
			if(accepted) begin
				previous_address <= vbuf_address[27:7];
				previous_address_valid <= 1'b1;
			end

			case({enqueue, dequeue})
				2'b10: begin
					if(fifo_count == 2'd0) begin
						fifo_address_token0 <= fold_address(vbuf_address);
						fifo_wrap0 <= accepted_wrap;
					end
					else begin
						fifo_address_token1 <= fold_address(vbuf_address);
						fifo_wrap1 <= accepted_wrap;
					end
					fifo_count <= fifo_count + 1'd1;
				end
				2'b01: begin
					if(fifo_count == 2'd2) begin
						fifo_address_token0 <= fifo_address_token1;
						fifo_wrap0 <= fifo_wrap1;
					end
					fifo_count <= fifo_count - 1'd1;
				end
				2'b11: begin
					if(fifo_count == 2'd1) begin
						fifo_address_token0 <= fold_address(vbuf_address);
						fifo_wrap0 <= accepted_wrap;
					end
					else begin
						fifo_address_token0 <= fifo_address_token1;
						fifo_wrap0 <= fifo_wrap1;
						fifo_address_token1 <= fold_address(vbuf_address);
						fifo_wrap1 <= accepted_wrap;
					end
				end
				default: begin end
			endcase

			if(return_has_entry) begin
				return_token = fold_return_data(vbuf_readdata) ^ TOKEN_DATA;

				if(return_phase == 7'd0 && fifo_wrap0) begin
					if(epoch_armed && !source_faulted) begin
						published_signature <= epoch_signature;
						published_flags <= FETCH_FLAG_CAPTURE_VALID;
						source_generation <= ~source_generation;
					end
					epoch_armed <= 1'b1;
					epoch_signature <= ordered_signature_update(
						ordered_signature_update(
							SIGNATURE_INITIAL, fifo_address_token0),
						return_token);
					fifo_wrap0 <= 1'b0;
				end
				else if(epoch_armed && !source_faulted) begin
					if(return_phase == 7'd0)
						epoch_signature <= ordered_signature_update(
							ordered_signature_update(
								epoch_signature, fifo_address_token0),
							return_token);
					else
						epoch_signature <= ordered_signature_update(
							epoch_signature, return_token);
				end

				if(return_phase == 7'd127)
					return_phase <= 7'd0;
				else
					return_phase <= return_phase + 1'd1;
			end

			if(!source_faulted && fault_event[6:1] != 6'd0) begin
				source_faulted <= 1'b1;
				epoch_armed <= 1'b0;
				published_signature <= 16'd0;
				published_flags <= fault_event;
				source_generation <= ~source_generation;
			end
		end
	end

	always @(*) begin
		if(word_count == MAGIK_RAW_SCALER_STATE_SCHEMA_WORD)
			response_word = FETCH_STATE_SCHEMA;
		else if(word_count == MAGIK_RAW_SCALER_STATE_FLAGS_WORD)
			response_word = {9'd0, snapshot_flags};
		else if(word_count == MAGIK_RAW_SCALER_STATE_CAPTURE_SEQUENCE_WORD)
			response_word = snapshot_sequence;
		else if(word_count == MAGIK_RAW_SCALER_STATE_CRC_WORD)
			response_word = tx_crc;
		else
			response_word = snapshot_signature;

		response_data = 16'd0;
		if(command_start && selected_start)
			response_data = MAGIK_RAW_SCALER_STATE_MAGIC;
		else if(command_data && selected_command &&
			(word_count < MAGIK_RAW_SCALER_STATE_WORDS))
			response_data = response_word;
	end

	// Stable bundled-data crossing. The destination waits one clk_sys edge after
	// observing the synchronized generation before capturing signature/flags.
	// A command snapshot remains immutable until io_uio is released.
	always @(posedge clk_sys or posedge reset_active) begin
		if(reset_active) begin
			generation_meta <= 1'b0;
			generation_sync <= 1'b0;
			generation_seen <= 1'b0;
			capture_pending <= 1'b0;
			snapshot_signature <= 16'd0;
			snapshot_sequence <= 16'd0;
			snapshot_flags <= 7'd0;
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
				snapshot_signature <= published_signature;
				snapshot_flags <= published_flags;
				if(published_flags == FETCH_FLAG_CAPTURE_VALID)
					snapshot_sequence <= snapshot_sequence + 1'd1;
				else begin
					snapshot_sequence <= 16'd0;
					snapshot_signature <= 16'd0;
				end
				capture_pending <= 1'b0;
			end

			if(command_start) begin
				has_command <= 1'b1;
				command_selected <= selected_start;
				word_count <= 3'd0;
				if(selected_start)
					tx_crc <= FETCH_SCHEMA_CRC;
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
