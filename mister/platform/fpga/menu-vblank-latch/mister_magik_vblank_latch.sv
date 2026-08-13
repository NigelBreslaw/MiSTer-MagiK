// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

`timescale 1ns/1ps
`default_nettype none

module mister_magik_vblank_latch (
	input  wire        clk_sys,
	input  wire        hdmi_vbl,
	input  wire        cmd_start,
	input  wire        cmd_data,
	input  wire [7:0]  cmd_id,
	input  wire [3:0]  word_index,
	input  wire [15:0] data_in,
	input  wire [15:0] evidence_word0,
	input  wire [15:0] evidence_word1,
	input  wire [15:0] evidence_word2,
	input  wire [15:0] evidence_word3,
	input  wire [15:0] evidence_word4,

	input  wire        active_lfb_en,
	input  wire [31:0] active_lfb_base,
	input  wire [11:0] active_lfb_width,
	input  wire [11:0] active_lfb_height,
	input  wire [13:0] active_lfb_stride,
	input  wire        apply_accepted,
	input  wire        legacy_write,

	output wire        response_valid,
	output reg  [15:0] response_data,
	output wire        apply,

	output reg         route_en = 1'b0,
	output reg         route_flt = 1'b0,
	output reg  [5:0]  route_fmt = 6'd0,
	output reg  [11:0] route_width = 12'd0,
	output reg  [11:0] route_height = 12'd0,
	output reg  [11:0] route_hmin = 12'd0,
	output reg  [11:0] route_hmax = 12'd0,
	output reg  [11:0] route_vmin = 12'd0,
	output reg  [11:0] route_vmax = 12'd0,
	output reg  [31:0] route_base = 32'd0,
	output reg  [13:0] route_stride = 14'd0,

	output reg         pending = 1'b0,
	output reg  [15:0] pending_seq = 16'd0,
	output reg  [15:0] active_seq = 16'd0,
	output reg  [15:0] post_count = 16'd0,
	output reg  [15:0] flip_count = 16'd0,
	output reg  [15:0] drop_count = 16'd0,
	output reg  [15:0] reject_count = 16'd0,
	output reg  [15:0] active_route_epoch = 16'd0
);

	`include "mister_magik_latch_protocol.svh"
	`include "mister_magik_video_diagnostics_protocol.svh"

	function automatic [15:0] crc_byte;
		input [15:0] current;
		input [7:0] value;
		integer bit_index;
		reg [15:0] next;
		begin
			next = current ^ {value, 8'h00};
			for(bit_index = 0; bit_index < 8; bit_index = bit_index + 1) begin
				if(next[15]) next = (next << 1) ^ MAGIK_CRC_POLYNOMIAL;
				else next = next << 1;
			end
			crc_byte = next;
		end
	endfunction

	function automatic [15:0] crc_word;
		input [15:0] current;
		input [15:0] value;
		begin
			crc_word = crc_byte(crc_byte(current, value[15:8]), value[7:0]);
		end
	endfunction

	function automatic [15:0] crc_header;
		input [7:0] command;
		input [15:0] non_crc_words;
		reg [15:0] next;
		begin
			next = crc_word(MAGIK_CRC_INITIAL, {8'd0, command});
			next = crc_word(next, MAGIK_FBUF_PROTOCOL_VERSION);
			crc_header = crc_word(next, non_crc_words);
		end
	endfunction

	reg rx_open = 1'b0;
	reg rx_faulted = 1'b0;
	reg [3:0] rx_expected = 4'd0;
	reg [10:0] rx_mask = 11'd0;
	reg [15:0] rx_crc = 16'd0;
	reg [15:0] rx_mode = 16'd0;
	reg [31:0] rx_base = 32'd0;
	reg [15:0] rx_width_word = 16'd0;
	reg [15:0] rx_height_word = 16'd0;
	reg [15:0] rx_hmin_word = 16'd0;
	reg [15:0] rx_hmax_word = 16'd0;
	reg [15:0] rx_vmin_word = 16'd0;
	reg [15:0] rx_vmax_word = 16'd0;
	reg [15:0] rx_stride_word = 16'd0;
	reg [15:0] rx_seq = 16'd0;
	reg [25:0] rx_row_span = 26'd0;
	reg rx_address_wrap = 1'b0;

	reg [3:0] last_reject_reason = MAGIK_REJECT_NONE;
	reg [15:0] last_reject_expected_index = 16'hffff;
	reg [15:0] last_reject_observed_index = 16'hffff;
	reg [15:0] last_reject_command = 16'd0;
	reg [15:0] last_reject_receiver_flags = 16'd0;
	reg magik_ownership = 1'b0;
	reg [15:0] attempted_transaction = 16'd0;
	reg [15:0] accepted_transaction = 16'd0;
	reg [15:0] pending_transaction = 16'd0;
	reg [15:0] active_transaction = 16'd0;
	reg [15:0] accepted_seq = 16'd0;
	reg [15:0] receipt_attempted_transaction = 16'd0;
	reg [15:0] receipt_attempted_sequence = 16'd0;
	reg [15:0] receipt_disposition = MAGIK_RECEIPT_NONE;
	reg [3:0] receipt_reject_reason = MAGIK_REJECT_NONE;
	reg [31:0] owned_vblank_count = 32'd0;
	reg [31:0] presented_vblank_count = 32'd0;
	reg [31:0] repeated_vblank_count = 32'd0;
	reg [31:0] ownership_loss_count = 32'd0;

	// Read commands are serialized, so one bank preserves each command-start
	// snapshot without carrying separate mutually exclusive register arrays.
	reg [15:0] response_snapshot [0:14];
	reg [15:0] tx_crc = 16'd0;
	reg [3:0] tx_expected = 4'd0;
	reg [7:0] tx_command = 8'd0;

	(* altera_attribute = "-name SYNCHRONIZER_IDENTIFICATION FORCED_IF_ASYNCHRONOUS" *)
	reg vbl_meta = 1'b0;
	(* altera_attribute = "-name SYNCHRONIZER_IDENTIFICATION FORCED_IF_ASYNCHRONOUS" *)
	reg vbl_sys = 1'b0;
	reg vbl_old = 1'b0;
	wire vbl_rise = ~vbl_old & vbl_sys;
	assign apply = pending && vbl_rise;

	wire [15:0] live_status_flags =
		({12'd0, last_reject_reason} << MAGIK_STATUS_REJECT_REASON_SHIFT) |
		(active_lfb_en ? (16'd1 << MAGIK_STATUS_ACTIVE_ENABLED) : 16'd0) |
		((pending && route_en) ? (16'd1 << MAGIK_STATUS_PENDING_ENABLED) : 16'd0) |
		(pending ? (16'd1 << MAGIK_STATUS_PENDING) : 16'd0) |
		(magik_ownership ? (16'd1 << MAGIK_STATUS_MAGIK_OWNERSHIP) : 16'd0);

	wire evidence_command =
		(cmd_id == MAGIK_UIO_GET_HDMI_EVIDENCE) ||
		(cmd_id == MAGIK_UIO_GET_HDMI_OUTPUT_ACTIVITY) ||
		(cmd_id == MAGIK_UIO_GET_HDMI_FINAL_PATH_ACTIVITY) ||
		(cmd_id == MAGIK_UIO_GET_HDMI_SCALER_RAW_ACTIVITY) ||
		(cmd_id == MAGIK_UIO_GET_HDMI_POST_OSD_ACTIVITY) ||
		(cmd_id == MAGIK_UIO_GET_HDMI_AVALON_LIVENESS_ACTIVITY) ||
		(cmd_id == MAGIK_UIO_GET_HDMI_SCALER_FETCH_ACTIVITY);
	wire [2:0] evidence_words =
		(cmd_id == MAGIK_UIO_GET_HDMI_EVIDENCE) ? MAGIK_HDMI_EVIDENCE_WORDS :
		(cmd_id == MAGIK_UIO_GET_HDMI_OUTPUT_ACTIVITY) ?
			MAGIK_HDMI_OUTPUT_ACTIVITY_WORDS :
		(cmd_id == MAGIK_UIO_GET_HDMI_FINAL_PATH_ACTIVITY) ?
			MAGIK_HDMI_FINAL_PATH_ACTIVITY_WORDS :
		(cmd_id == MAGIK_UIO_GET_HDMI_SCALER_RAW_ACTIVITY) ?
			MAGIK_HDMI_SCALER_RAW_ACTIVITY_WORDS :
		(cmd_id == MAGIK_UIO_GET_HDMI_POST_OSD_ACTIVITY) ?
			MAGIK_HDMI_POST_OSD_ACTIVITY_WORDS :
		(cmd_id == MAGIK_UIO_GET_HDMI_AVALON_LIVENESS_ACTIVITY) ?
			MAGIK_HDMI_AVALON_LIVENESS_ACTIVITY_WORDS :
		(cmd_id == MAGIK_UIO_GET_HDMI_SCALER_FETCH_ACTIVITY) ?
			MAGIK_HDMI_SCALER_FETCH_ACTIVITY_WORDS : 3'd0;
	wire [2:0] evidence_crc_word = evidence_words - 1'd1;
	reg [15:0] evidence_magic;
	reg [15:0] evidence_header_crc;
	always @(*) begin
		evidence_magic = 16'd0;
		evidence_header_crc = 16'd0;
		case(cmd_id)
			MAGIK_UIO_GET_HDMI_EVIDENCE: begin
				evidence_magic = MAGIK_HDMI_EVIDENCE_MAGIC;
				evidence_header_crc = MAGIK_HDMI_EVIDENCE_HEADER_CRC;
			end
			MAGIK_UIO_GET_HDMI_OUTPUT_ACTIVITY: begin
				evidence_magic = MAGIK_HDMI_OUTPUT_ACTIVITY_MAGIC;
				evidence_header_crc = MAGIK_HDMI_OUTPUT_ACTIVITY_HEADER_CRC;
			end
			MAGIK_UIO_GET_HDMI_FINAL_PATH_ACTIVITY: begin
				evidence_magic = MAGIK_HDMI_FINAL_PATH_ACTIVITY_MAGIC;
				evidence_header_crc = MAGIK_HDMI_FINAL_PATH_ACTIVITY_HEADER_CRC;
			end
			MAGIK_UIO_GET_HDMI_SCALER_RAW_ACTIVITY: begin
				evidence_magic = MAGIK_HDMI_SCALER_RAW_ACTIVITY_MAGIC;
				evidence_header_crc = MAGIK_HDMI_SCALER_RAW_ACTIVITY_HEADER_CRC;
			end
			MAGIK_UIO_GET_HDMI_POST_OSD_ACTIVITY: begin
				evidence_magic = MAGIK_HDMI_POST_OSD_ACTIVITY_MAGIC;
				evidence_header_crc = MAGIK_HDMI_POST_OSD_ACTIVITY_HEADER_CRC;
			end
			MAGIK_UIO_GET_HDMI_AVALON_LIVENESS_ACTIVITY: begin
				evidence_magic = MAGIK_HDMI_AVALON_LIVENESS_ACTIVITY_MAGIC;
				evidence_header_crc = MAGIK_HDMI_AVALON_LIVENESS_ACTIVITY_HEADER_CRC;
			end
			MAGIK_UIO_GET_HDMI_SCALER_FETCH_ACTIVITY: begin
				evidence_magic = MAGIK_HDMI_SCALER_FETCH_ACTIVITY_MAGIC;
				evidence_header_crc = MAGIK_HDMI_SCALER_FETCH_ACTIVITY_HEADER_CRC;
			end
			default: begin end
		endcase
	end

	wire rx_reserved_fields =
		(|rx_width_word[15:12]) || (|rx_height_word[15:12]) ||
		(|rx_hmin_word[15:12]) || (|rx_hmax_word[15:12]) ||
		(|rx_vmin_word[15:12]) || (|rx_vmax_word[15:12]) ||
		(|rx_stride_word[15:14]);
	wire [11:0] rx_width = rx_width_word[11:0];
	wire [11:0] rx_height = rx_height_word[11:0];
	wire [11:0] rx_hmin = rx_hmin_word[11:0];
	wire [11:0] rx_hmax = rx_hmax_word[11:0];
	wire [11:0] rx_vmin = rx_vmin_word[11:0];
	wire [11:0] rx_vmax = rx_vmax_word[11:0];
	wire [13:0] rx_stride = rx_stride_word[13:0];
	wire [11:0] rx_height_minus_one = rx_height - 12'd1;
	wire [25:0] rx_next_row_span =
		rx_height_minus_one * data_in[13:0];
	wire [32:0] rx_pipelined_end_address =
		{1'b0, rx_base} +
		{{7{1'b0}}, rx_row_span} +
		{{20{1'b0}}, rx_width, 1'b0};

	reg [3:0] semantic_reject;
	always @(*) begin
		semantic_reject = MAGIK_REJECT_NONE;
		if(!rx_mode[15]) begin
			if((rx_mode != 16'd0) || (rx_base != 32'd0) ||
			   (rx_width_word != 16'd0) || (rx_height_word != 16'd0) ||
			   (rx_hmin_word != 16'd0) || (rx_hmax_word != 16'd0) ||
			   (rx_vmin_word != 16'd0) || (rx_vmax_word != 16'd0) ||
			   (rx_stride_word != 16'd0))
				semantic_reject = MAGIK_REJECT_INVALID_MODE;
		end
		else if((|rx_mode[13:6]) || (rx_mode[5:0] != 6'h14))
			semantic_reject = MAGIK_REJECT_INVALID_MODE;
		else if(rx_reserved_fields)
			semantic_reject = MAGIK_REJECT_RESERVED;
		else if((rx_base == 32'd0) || rx_base[0])
			semantic_reject = MAGIK_REJECT_INVALID_BASE;
		else if((rx_width == 0) ||
		        ({4'd0, rx_width} > MAGIK_FBUF_MAX_WIDTH) ||
		        (rx_height == 0) ||
		        ({4'd0, rx_height} > MAGIK_FBUF_MAX_HEIGHT))
			semantic_reject = MAGIK_REJECT_INVALID_GEOMETRY;
		else if(rx_stride[0] || (rx_stride < ({2'd0, rx_width} << 1)) ||
		        ({2'd0, rx_stride} > MAGIK_FBUF_MAX_STRIDE))
			semantic_reject = MAGIK_REJECT_INVALID_STRIDE;
		else if((rx_hmin > rx_hmax) || (rx_vmin > rx_vmax))
			semantic_reject = MAGIK_REJECT_INVALID_BOUNDS;
		else if(rx_address_wrap)
			semantic_reject = MAGIK_REJECT_ADDRESS_WRAP;
	end

	assign response_valid =
		(cmd_start && ((cmd_id == MAGIK_UIO_SET_FBUF_LATCH) ||
		               (cmd_id == MAGIK_UIO_GET_FBUF_LATCH) ||
		               (cmd_id == MAGIK_UIO_GET_FBUF_LATCH_CAPS) ||
		               (cmd_id == MAGIK_UIO_GET_FBUF_LATCH_DIAGNOSTICS) ||
		               (cmd_id == MAGIK_UIO_GET_FBUF_LATCH_RECEIPT) ||
		               (cmd_id == MAGIK_UIO_GET_FBUF_PRESENTATION_TELEMETRY) ||
		               evidence_command)) ||
		(cmd_data && ((cmd_id == MAGIK_UIO_GET_FBUF_LATCH) ||
		              (cmd_id == MAGIK_UIO_GET_FBUF_LATCH_CAPS) ||
		              (cmd_id == MAGIK_UIO_GET_FBUF_LATCH_DIAGNOSTICS) ||
		              (cmd_id == MAGIK_UIO_GET_FBUF_LATCH_RECEIPT) ||
		              (cmd_id == MAGIK_UIO_GET_FBUF_PRESENTATION_TELEMETRY) ||
		              (evidence_command && (word_index < evidence_words))));

	always @(*) begin
		response_data = 16'd0;
		if(cmd_start) begin
			case(cmd_id)
				MAGIK_UIO_SET_FBUF_LATCH: response_data = MAGIK_FBUF_LATCH_MAGIC;
				MAGIK_UIO_GET_FBUF_LATCH: response_data = MAGIK_FBUF_STATUS_MAGIC;
				MAGIK_UIO_GET_FBUF_LATCH_CAPS: response_data = MAGIK_FBUF_CAPS_MAGIC;
				MAGIK_UIO_GET_FBUF_LATCH_DIAGNOSTICS:
					response_data = MAGIK_FBUF_DIAGNOSTICS_MAGIC;
				MAGIK_UIO_GET_FBUF_LATCH_RECEIPT:
					response_data = MAGIK_FBUF_RECEIPT_MAGIC;
				MAGIK_UIO_GET_FBUF_PRESENTATION_TELEMETRY:
					response_data = MAGIK_FBUF_PRESENTATION_TELEMETRY_MAGIC;
				MAGIK_UIO_GET_HDMI_EVIDENCE,
				MAGIK_UIO_GET_HDMI_OUTPUT_ACTIVITY,
				MAGIK_UIO_GET_HDMI_FINAL_PATH_ACTIVITY,
				MAGIK_UIO_GET_HDMI_SCALER_RAW_ACTIVITY,
				MAGIK_UIO_GET_HDMI_POST_OSD_ACTIVITY,
				MAGIK_UIO_GET_HDMI_AVALON_LIVENESS_ACTIVITY,
				MAGIK_UIO_GET_HDMI_SCALER_FETCH_ACTIVITY:
					response_data = evidence_magic;
				default: response_data = 16'd0;
			endcase
		end
		else if(cmd_data && (cmd_id == MAGIK_UIO_GET_FBUF_LATCH)) begin
			if(word_index < 4'd15) response_data = response_snapshot[word_index];
			else if(word_index == 4'd15)
				response_data = tx_crc ^ MAGIK_CRC_FINAL_XOR;
		end
		else if(cmd_data && (cmd_id == MAGIK_UIO_GET_FBUF_LATCH_CAPS)) begin
			case(word_index)
				4'd0: response_data = MAGIK_FBUF_PROTOCOL_VERSION;
				4'd1: response_data = MAGIK_FBUF_CAPS_FLAGS;
				4'd2: response_data = MAGIK_FBUF_MAX_WIDTH;
				4'd3: response_data = MAGIK_FBUF_MAX_HEIGHT;
				4'd4: response_data = MAGIK_FBUF_MAX_STRIDE;
				4'd5: response_data = tx_crc ^ MAGIK_CRC_FINAL_XOR;
				default: response_data = 16'd0;
			endcase
		end
		else if(cmd_data && (cmd_id == MAGIK_UIO_GET_FBUF_LATCH_DIAGNOSTICS)) begin
			if(word_index < 4'd6) response_data = response_snapshot[word_index];
			else if(word_index == 4'd6)
				response_data = tx_crc ^ MAGIK_CRC_FINAL_XOR;
		end
		else if(cmd_data && (cmd_id == MAGIK_UIO_GET_FBUF_LATCH_RECEIPT)) begin
			if(word_index < 4'd10) response_data = response_snapshot[word_index];
			else if(word_index == 4'd10)
				response_data = tx_crc ^ MAGIK_CRC_FINAL_XOR;
		end
		else if(cmd_data && (cmd_id == MAGIK_UIO_GET_FBUF_PRESENTATION_TELEMETRY)) begin
			if(word_index < 4'd10) response_data = response_snapshot[word_index];
			else if(word_index == 4'd10)
				response_data = tx_crc ^ MAGIK_CRC_FINAL_XOR;
		end
		else if(cmd_data && evidence_command) begin
			if(word_index < evidence_crc_word)
				response_data = response_snapshot[word_index];
			else if(word_index == evidence_crc_word)
				response_data = tx_crc ^ MAGIK_CRC_FINAL_XOR;
		end
	end

	always @(posedge clk_sys) begin
		vbl_meta <= hdmi_vbl;
		vbl_sys <= vbl_meta;
		vbl_old <= vbl_sys;

		if(cmd_start && (cmd_id == MAGIK_UIO_GET_FBUF_LATCH)) begin
			response_snapshot[0] <= active_seq;
			response_snapshot[1] <= pending_seq;
			response_snapshot[2] <= live_status_flags;
			response_snapshot[3] <= flip_count;
			response_snapshot[4] <= post_count;
			response_snapshot[5] <= active_lfb_base[15:0];
			response_snapshot[6] <= active_lfb_base[31:16];
			response_snapshot[7] <= {4'd0, active_lfb_width};
			response_snapshot[8] <= {4'd0, active_lfb_height};
			response_snapshot[9] <= {2'd0, active_lfb_stride};
			response_snapshot[10] <= reject_count;
			response_snapshot[11] <= active_route_epoch;
			response_snapshot[12] <= active_transaction;
			response_snapshot[13] <= pending_transaction;
			response_snapshot[14] <= accepted_transaction;
			tx_crc <= crc_header(MAGIK_UIO_GET_FBUF_LATCH, 16'd15);
			tx_expected <= 4'd0;
			tx_command <= MAGIK_UIO_GET_FBUF_LATCH;
		end
		else if(cmd_start && (cmd_id == MAGIK_UIO_GET_FBUF_LATCH_CAPS)) begin
			tx_crc <= crc_header(MAGIK_UIO_GET_FBUF_LATCH_CAPS, 16'd5);
			tx_expected <= 4'd0;
			tx_command <= MAGIK_UIO_GET_FBUF_LATCH_CAPS;
		end
		else if(cmd_start && (cmd_id == MAGIK_UIO_GET_FBUF_LATCH_DIAGNOSTICS)) begin
			response_snapshot[0] <= reject_count;
			response_snapshot[1] <= {12'd0, last_reject_reason};
			response_snapshot[2] <= last_reject_expected_index;
			response_snapshot[3] <= last_reject_observed_index;
			response_snapshot[4] <= last_reject_command;
			response_snapshot[5] <= last_reject_receiver_flags;
			tx_crc <= crc_header(MAGIK_UIO_GET_FBUF_LATCH_DIAGNOSTICS, 16'd6);
			tx_expected <= 4'd0;
			tx_command <= MAGIK_UIO_GET_FBUF_LATCH_DIAGNOSTICS;
		end
		else if(cmd_start && (cmd_id == MAGIK_UIO_GET_FBUF_LATCH_RECEIPT)) begin
			response_snapshot[0] <= rx_open ? attempted_transaction :
				receipt_attempted_transaction;
			response_snapshot[1] <= rx_open ? rx_seq : receipt_attempted_sequence;
			response_snapshot[2] <= rx_open ? MAGIK_RECEIPT_REJECTED :
				receipt_disposition;
			response_snapshot[3] <= accepted_transaction;
			response_snapshot[4] <= accepted_seq;
			response_snapshot[5] <= pending_transaction;
			response_snapshot[6] <= pending_seq;
			response_snapshot[7] <= active_transaction;
			response_snapshot[8] <= active_seq;
			response_snapshot[9] <= rx_open ? {12'd0, MAGIK_REJECT_MISSING_WORD} :
				{12'd0, receipt_reject_reason};
			tx_crc <= crc_header(MAGIK_UIO_GET_FBUF_LATCH_RECEIPT, 16'd10);
			tx_expected <= 4'd0;
			tx_command <= MAGIK_UIO_GET_FBUF_LATCH_RECEIPT;
		end
		else if(cmd_start && (cmd_id == MAGIK_UIO_GET_FBUF_PRESENTATION_TELEMETRY)) begin
			response_snapshot[0] <= owned_vblank_count[15:0];
			response_snapshot[1] <= owned_vblank_count[31:16];
			response_snapshot[2] <= presented_vblank_count[15:0];
			response_snapshot[3] <= presented_vblank_count[31:16];
			response_snapshot[4] <= repeated_vblank_count[15:0];
			response_snapshot[5] <= repeated_vblank_count[31:16];
			response_snapshot[6] <= ownership_loss_count[15:0];
			response_snapshot[7] <= ownership_loss_count[31:16];
			response_snapshot[8] <= active_seq;
			response_snapshot[9] <= live_status_flags;
			tx_crc <= crc_header(MAGIK_UIO_GET_FBUF_PRESENTATION_TELEMETRY, 16'd10);
			tx_expected <= 4'd0;
			tx_command <= MAGIK_UIO_GET_FBUF_PRESENTATION_TELEMETRY;
		end
		else if(cmd_start && evidence_command) begin
			response_snapshot[0] <= evidence_word0;
			response_snapshot[1] <= evidence_word1;
			response_snapshot[2] <= evidence_word2;
			response_snapshot[3] <= evidence_word3;
			response_snapshot[4] <= evidence_word4;
			tx_crc <= evidence_header_crc;
			tx_expected <= 4'd0;
			tx_command <= cmd_id;
		end
		else if(cmd_data && (cmd_id == tx_command) &&
		        (word_index == tx_expected)) begin
			if((tx_command == MAGIK_UIO_GET_FBUF_LATCH) && (word_index < 4'd15)) begin
				tx_crc <= crc_word(tx_crc, response_snapshot[word_index]);
				tx_expected <= tx_expected + 1'd1;
			end
				else if((tx_command == MAGIK_UIO_GET_FBUF_LATCH_CAPS) &&
				        (word_index < 4'd5)) begin
				case(word_index)
					4'd0: tx_crc <= crc_word(tx_crc, MAGIK_FBUF_PROTOCOL_VERSION);
					4'd1: tx_crc <= crc_word(tx_crc, MAGIK_FBUF_CAPS_FLAGS);
					4'd2: tx_crc <= crc_word(tx_crc, MAGIK_FBUF_MAX_WIDTH);
					4'd3: tx_crc <= crc_word(tx_crc, MAGIK_FBUF_MAX_HEIGHT);
					4'd4: tx_crc <= crc_word(tx_crc, MAGIK_FBUF_MAX_STRIDE);
					// The enclosing range check makes this defensive arm unreachable.
					/* verilator coverage_off */
					default: tx_crc <= tx_crc;
					/* verilator coverage_on */
				endcase
					tx_expected <= tx_expected + 1'd1;
				end
				else if((tx_command == MAGIK_UIO_GET_FBUF_LATCH_DIAGNOSTICS) &&
				        (word_index < 4'd6)) begin
					tx_crc <= crc_word(tx_crc, response_snapshot[word_index]);
					tx_expected <= tx_expected + 1'd1;
				end
				else if((tx_command == MAGIK_UIO_GET_FBUF_LATCH_RECEIPT) &&
				        (word_index < 4'd10)) begin
					tx_crc <= crc_word(tx_crc, response_snapshot[word_index]);
					tx_expected <= tx_expected + 1'd1;
				end
				else if((tx_command == MAGIK_UIO_GET_FBUF_PRESENTATION_TELEMETRY) &&
				        (word_index < 4'd10)) begin
					tx_crc <= crc_word(tx_crc, response_snapshot[word_index]);
					tx_expected <= tx_expected + 1'd1;
				end
				else if(evidence_command && (word_index < evidence_crc_word)) begin
					tx_crc <= crc_word(tx_crc, response_snapshot[word_index]);
					tx_expected <= tx_expected + 1'd1;
				end
			end

		if(legacy_write && magik_ownership)
			ownership_loss_count <= ownership_loss_count + 1'd1;
		if(vbl_rise && !legacy_write) begin
			if(apply_accepted) begin
				owned_vblank_count <= owned_vblank_count + 1'd1;
				presented_vblank_count <= presented_vblank_count + 1'd1;
			end
			else if(magik_ownership) begin
				owned_vblank_count <= owned_vblank_count + 1'd1;
				repeated_vblank_count <= repeated_vblank_count + 1'd1;
			end
		end

		if(legacy_write) begin
			magik_ownership <= 1'b0;
			active_seq <= 16'd0;
			active_transaction <= 16'd0;
			accepted_seq <= 16'd0;
			accepted_transaction <= 16'd0;
			pending_seq <= 16'd0;
			pending_transaction <= 16'd0;
			pending <= 1'b0;
			active_route_epoch <= active_route_epoch + 1'd1;
		end
		else if(apply_accepted) begin
			magik_ownership <= 1'b1;
			active_seq <= pending_seq;
			active_transaction <= pending_transaction;
			pending_seq <= 16'd0;
			pending_transaction <= 16'd0;
			active_route_epoch <= active_route_epoch + 1'd1;
			flip_count <= flip_count + 1'd1;
			pending <= 1'b0;
		end

		if(cmd_start) begin
			if(rx_open) begin
				reject_count <= reject_count + 1'd1;
				last_reject_expected_index <= {12'd0, rx_expected};
				last_reject_observed_index <= 16'd0;
				last_reject_command <= {8'd0, cmd_id};
				last_reject_receiver_flags <= {14'd0, rx_faulted, rx_open};
				if(cmd_id == MAGIK_UIO_SET_FBUF_LATCH)
					last_reject_reason <= MAGIK_REJECT_RESTARTED;
				else
					last_reject_reason <= MAGIK_REJECT_MISSING_WORD;
				receipt_attempted_transaction <= attempted_transaction;
				receipt_attempted_sequence <= rx_seq;
				receipt_disposition <= MAGIK_RECEIPT_REJECTED;
				receipt_reject_reason <= (cmd_id == MAGIK_UIO_SET_FBUF_LATCH) ?
					MAGIK_REJECT_RESTARTED : MAGIK_REJECT_MISSING_WORD;
			end
			if(cmd_id == MAGIK_UIO_SET_FBUF_LATCH) begin
				attempted_transaction <= attempted_transaction + 1'd1;
				receipt_attempted_transaction <= attempted_transaction + 1'd1;
				receipt_attempted_sequence <= 16'd0;
				receipt_disposition <= MAGIK_RECEIPT_NONE;
				receipt_reject_reason <= MAGIK_REJECT_NONE;
				rx_open <= 1'b1;
				rx_faulted <= 1'b0;
				rx_seq <= 16'd0;
				rx_expected <= 4'd0;
				rx_mask <= 11'd0;
				rx_crc <= crc_header(MAGIK_UIO_SET_FBUF_LATCH, 16'd11);
				rx_row_span <= 26'd0;
				rx_address_wrap <= 1'b0;
			end
			else begin
				rx_open <= 1'b0;
				rx_faulted <= 1'b1;
			end
		end
		else if(cmd_data && (cmd_id == MAGIK_UIO_SET_FBUF_LATCH)) begin
			if(!rx_open) begin
				if(!rx_faulted) begin
					reject_count <= reject_count + 1'd1;
					last_reject_reason <= MAGIK_REJECT_POST_CLOSE;
					receipt_disposition <= MAGIK_RECEIPT_REJECTED;
					receipt_reject_reason <= MAGIK_REJECT_POST_CLOSE;
					last_reject_expected_index <= {12'd0, rx_expected};
					last_reject_observed_index <= {12'd0, word_index};
					last_reject_command <= {8'd0, cmd_id};
					last_reject_receiver_flags <= {14'd0, rx_faulted, rx_open};
					rx_faulted <= 1'b1;
				end
			end
			else if((word_index < rx_expected) &&
			        (word_index + 1'd1 == rx_expected)) begin
				reject_count <= reject_count + 1'd1;
				last_reject_reason <= MAGIK_REJECT_DUPLICATE_WORD;
				receipt_disposition <= MAGIK_RECEIPT_REJECTED;
				receipt_reject_reason <= MAGIK_REJECT_DUPLICATE_WORD;
				last_reject_expected_index <= {12'd0, rx_expected};
				last_reject_observed_index <= {12'd0, word_index};
				last_reject_command <= {8'd0, cmd_id};
				last_reject_receiver_flags <= {14'd0, rx_faulted, rx_open};
				rx_open <= 1'b0;
				rx_faulted <= 1'b1;
			end
			else if(word_index < rx_expected) begin
				reject_count <= reject_count + 1'd1;
				last_reject_reason <= MAGIK_REJECT_OUT_OF_ORDER;
				receipt_disposition <= MAGIK_RECEIPT_REJECTED;
				receipt_reject_reason <= MAGIK_REJECT_OUT_OF_ORDER;
				last_reject_expected_index <= {12'd0, rx_expected};
				last_reject_observed_index <= {12'd0, word_index};
				last_reject_command <= {8'd0, cmd_id};
				last_reject_receiver_flags <= {14'd0, rx_faulted, rx_open};
				rx_open <= 1'b0;
				rx_faulted <= 1'b1;
			end
			else if((word_index == 4'd11) && (rx_expected != 4'd11)) begin
				reject_count <= reject_count + 1'd1;
				last_reject_reason <= MAGIK_REJECT_MISSING_WORD;
				receipt_disposition <= MAGIK_RECEIPT_REJECTED;
				receipt_reject_reason <= MAGIK_REJECT_MISSING_WORD;
				last_reject_expected_index <= {12'd0, rx_expected};
				last_reject_observed_index <= {12'd0, word_index};
				last_reject_command <= {8'd0, cmd_id};
				last_reject_receiver_flags <= {14'd0, rx_faulted, rx_open};
				rx_open <= 1'b0;
				rx_faulted <= 1'b1;
			end
			else if(word_index > rx_expected) begin
				reject_count <= reject_count + 1'd1;
				last_reject_reason <= MAGIK_REJECT_SHIFTED_WORD;
				receipt_disposition <= MAGIK_RECEIPT_REJECTED;
				receipt_reject_reason <= MAGIK_REJECT_SHIFTED_WORD;
				last_reject_expected_index <= {12'd0, rx_expected};
				last_reject_observed_index <= {12'd0, word_index};
				last_reject_command <= {8'd0, cmd_id};
				last_reject_receiver_flags <= {14'd0, rx_faulted, rx_open};
				rx_open <= 1'b0;
				rx_faulted <= 1'b1;
			end
			else if(word_index < 4'd11) begin
				rx_mask[word_index] <= 1'b1;
				rx_crc <= crc_word(rx_crc, data_in);
				rx_expected <= rx_expected + 1'd1;
				case(word_index)
					4'd0: rx_mode <= data_in;
					4'd1: rx_base[15:0] <= data_in;
					4'd2: rx_base[31:16] <= data_in;
					4'd3: rx_width_word <= data_in;
					4'd4: rx_height_word <= data_in;
					4'd5: rx_hmin_word <= data_in;
					4'd6: rx_hmax_word <= data_in;
					4'd7: rx_vmin_word <= data_in;
					4'd8: rx_vmax_word <= data_in;
					4'd9: begin
						rx_stride_word <= data_in;
						rx_row_span <= rx_next_row_span;
					end
					4'd10: begin
						rx_seq <= data_in;
						receipt_attempted_sequence <= data_in;
						rx_address_wrap <=
							rx_pipelined_end_address > 33'h100000000;
					end
					// word_index < 11 and exact ordering restrict this case to 0..10.
					/* verilator coverage_off */
					default: begin end
					/* verilator coverage_on */
				endcase
			end
			else begin
				rx_open <= 1'b0;
				rx_faulted <= 1'b0;
				// Exact in-order framing makes a CRC commit reachable only after all
				// eleven payload bits are set. Keep the mask check as defense in depth.
				/* verilator coverage_off */
				if(rx_mask != 11'h7ff) begin
					reject_count <= reject_count + 1'd1;
					last_reject_reason <= MAGIK_REJECT_MISSING_WORD;
					receipt_disposition <= MAGIK_RECEIPT_REJECTED;
					receipt_reject_reason <= MAGIK_REJECT_MISSING_WORD;
					last_reject_expected_index <= {12'd0, rx_expected};
					last_reject_observed_index <= {12'd0, word_index};
					last_reject_command <= {8'd0, cmd_id};
					last_reject_receiver_flags <= {14'd0, rx_faulted, rx_open};
					rx_faulted <= 1'b1;
				end
				/* verilator coverage_on */
				else if(data_in != (rx_crc ^ MAGIK_CRC_FINAL_XOR)) begin
					reject_count <= reject_count + 1'd1;
					last_reject_reason <= MAGIK_REJECT_BAD_CRC;
					receipt_disposition <= MAGIK_RECEIPT_REJECTED;
					receipt_reject_reason <= MAGIK_REJECT_BAD_CRC;
					last_reject_expected_index <= {12'd0, rx_expected};
					last_reject_observed_index <= {12'd0, word_index};
					last_reject_command <= {8'd0, cmd_id};
					last_reject_receiver_flags <= {14'd0, rx_faulted, rx_open};
					rx_faulted <= 1'b1;
				end
				else if(pending) begin
					reject_count <= reject_count + 1'd1;
					drop_count <= drop_count + 1'd1;
					last_reject_reason <= MAGIK_REJECT_PENDING_BUSY;
					last_reject_expected_index <= {12'd0, rx_expected};
					last_reject_observed_index <= {12'd0, word_index};
					last_reject_command <= {8'd0, cmd_id};
					last_reject_receiver_flags <= {14'd0, rx_faulted, rx_open};
					receipt_disposition <= MAGIK_RECEIPT_REJECTED;
					receipt_reject_reason <= MAGIK_REJECT_PENDING_BUSY;
					rx_faulted <= 1'b1;
				end
				else if(semantic_reject != MAGIK_REJECT_NONE) begin
					reject_count <= reject_count + 1'd1;
					last_reject_reason <= semantic_reject;
					receipt_disposition <= MAGIK_RECEIPT_REJECTED;
					receipt_reject_reason <= semantic_reject;
					last_reject_expected_index <= {12'd0, rx_expected};
					last_reject_observed_index <= {12'd0, word_index};
					last_reject_command <= {8'd0, cmd_id};
					last_reject_receiver_flags <= {14'd0, rx_faulted, rx_open};
					rx_faulted <= 1'b1;
				end
				else begin
					route_en <= rx_mode[15];
					route_flt <= rx_mode[14];
					route_fmt <= rx_mode[5:0];
					route_base <= rx_base;
					route_width <= rx_width;
					route_height <= rx_height;
					route_hmin <= rx_hmin;
					route_hmax <= rx_hmax;
					route_vmin <= rx_vmin;
					route_vmax <= rx_vmax;
					route_stride <= rx_stride;
					pending_seq <= rx_seq;
					pending_transaction <= attempted_transaction;
					accepted_transaction <= attempted_transaction;
					accepted_seq <= rx_seq;
					pending <= 1'b1;
					post_count <= post_count + 1'd1;
					last_reject_reason <= MAGIK_REJECT_NONE;
					receipt_disposition <= MAGIK_RECEIPT_ACCEPTED;
					receipt_reject_reason <= MAGIK_REJECT_NONE;
				end
			end
		end
	end

	`ifndef SYNTHESIS
	always @(posedge clk_sys) begin
		assert(owned_vblank_count == (presented_vblank_count + repeated_vblank_count))
			else $fatal(1, "owned vblank accounting invariant failed");
		if(!pending) begin
			assert(accepted_seq == active_seq)
				else $fatal(1, "accepted N / active N-1 / no pending is forbidden");
			assert(accepted_transaction == active_transaction)
				else $fatal(1, "active transaction must originate from the accepted receipt");
		end
	end
	`endif

endmodule

`default_nettype wire
